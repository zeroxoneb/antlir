#!/usr/bin/env python3
'''
"Atomically" [1] downloads a snapshot of a sequence of RPM repos. Uses the
`repo_db.py` and `storage.py` abstractions to store the snapshots, while
avoiding duplication of RPMs that existed in prior snapshots.

Specifically, the user calls `download_repos(...)`, which, for each repo:
  - Downloads & parses `repomd.xml`.
  - Downloads the repodatas referenced there. Parses a primary repodata.
  - Downloads the RPMs referenced in the primary repodata.

To increase performance, each of the above steps is performed concurrently,
with `download_repos` being the driver that aggregates the thread results and
returns the final list of snapshots. Additionally, the single driver thread
performs all writes to mitigate potential concurrency issues with SQLite.

`download_repos` returns a list of `RepoSnapshot`s containing descriptions of
the stored objects. The dictionary keys are either "storage IDs" from the
supplied `Storage` class, or `ReportableError` instances for those that were
not correctly downloaded and stored. If a download fatally fails for a
particular repo (e.g. repomd download failed, or a primary repodata couldn't be
retrieved), the exception will be raised and the entire snapshot will fail.

Note that in the case of the snapshot failing part way through, we omit any
complex logic to clean up objects that have already been committed. This is
mainly because until the very last point of the snapshot, where repomds are
committed, these inserted repo objects should be unreferenced, and thus will
essentially just be taking up extra space in storage without causing any
integrity issues. Additionally, if this leaking becomes substantial, it's
possible to simply have a periodic clean-up job run which garbage collects any
unreferenced blobs - which is a much simpler approach compared to ensuring we
always clean up unfinished work.

[1] The snapshot is only atomic (i.e. representative of a single point in time,
as opposed to a sheared mix of the repo at various points in time) if:
  - Repodata files and RPM files are never mutated after creation. For
    repodata, this is plausible because their names include their hash.  For
    RPMs, this code includes a "mutable RPM" guard to detect files whose
    contents changed.
  - `repomd.xml` is replaced atomically (i.e.  via `rename`) after making
    available all the new RPMs & repodatas.
'''
from typing import Iterable, Iterator, Tuple

from fs_image.common import get_file_logger
from fs_image.rpm.downloader.common import DownloadConfig
from fs_image.rpm.downloader.repomd_downloader import download_repomds
from fs_image.rpm.downloader.repodata_downloader import (
    get_repodatas_from_repomds
)
from fs_image.rpm.downloader.rpm_downloader import get_rpms_from_repodatas
from rpm.repo_db import RepoDBContext
from rpm.repo_snapshot import RepoSnapshot
from rpm.yum_dnf_conf import YumDnfConfRepo

log = get_file_logger(__file__)


def download_repos(
    repos_and_universes: Iterable[Tuple[YumDnfConfRepo, str]],
    *,
    cfg: DownloadConfig,
    visitors: Iterable['RepoObjectVisitor'] = (),
) -> Iterator[Tuple[YumDnfConfRepo, RepoSnapshot]]:
    all_snapshot_universes = frozenset(u for _, u in repos_and_universes)
    db_conn = cfg.new_db_conn()
    db_ctx = RepoDBContext(db_conn, db_conn.SQL_DIALECT)
    with db_ctx as repo_db_ctx:
        repo_db_ctx.ensure_tables_exist()

    # Concurrently download repomds, aggregate results
    repomd_results = download_repomds(
        repos_and_universes, cfg, visitors
    )
    repodata_results = get_repodatas_from_repomds(
        repomd_results, cfg, visitors
    )
    # Cast to run the generators before storing into the db
    rpm_results = list(get_rpms_from_repodatas(
        repodata_results, cfg, visitors, all_snapshot_universes
    ))

    # All downloads have completed - we now want to atomically persist repomds.
    with db_ctx as repo_db_ctx:
        # Even though a valid snapshot of a single repo is intrinsically valid,
        # we only want to operate on coherent collections of repos (as they
        # existed at roughly the same point in time). For this reason, we'd
        # rather leak already-committed repodata & RPM objects (subject to GC
        # later, if we choose) if we were not able to store a full snapshot,
        # while not doing so for repomds (as committing those essentially
        # commits a full snasphot, given that the repodata & RPM objects will
        # now be referenced).
        for res in rpm_results:
            repo_db_ctx.store_repomd(
                res.repo_universe, res.repo.name, res.repomd
            )
        try:
            repo_db_ctx.commit()
        except Exception:  # pragma: no cover
            # This is bad, but we hope this commit was atomic and thus none of
            # the repomds got inserted, in which case our snapshot's failed but
            # we at least don't have a semi-complete snapshot in the db.
            log.exception(f'Exception when trying to commit repomd')
            raise

    return (
        (
            res.repo,
            RepoSnapshot(
                repomd=res.repomd,
                storage_id_to_repodata=res.storage_id_to_repodata,
                storage_id_to_rpm=res.storage_id_to_rpm,
            )
        )
        for res in rpm_results
    )
