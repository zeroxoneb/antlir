/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::collections::HashMap;
use std::fmt::Display;

use configparser::ini::Ini;
use http::Uri;
use itertools::Itertools;

#[derive(Debug, Clone, Default)]
pub struct DnfConf {
    repos: HashMap<String, RepoConf>,
}

impl DnfConf {
    pub fn builder() -> DnfConfBuilder {
        DnfConfBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            repos: HashMap::new(),
        }
    }

    pub fn add_repo(&mut self, id: String, repo_cfg: RepoConf) {
        self.repos.insert(id, repo_cfg);
    }
}

impl Display for DnfConf {
    #[deny(unused_variables)]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let Self { repos } = self;
        let mut config = Ini::new();
        for (id, repo) in repos {
            config.set(
                &id.replace('/', "-"),
                "baseurl",
                Some(
                    repo.base_urls
                        .iter()
                        .map(|u| u.to_string())
                        .join("\n        "),
                ),
            );
            if let Some(name) = &repo.name {
                config.set(id, "name", Some(name.clone()));
            }
        }
        write!(f, "{}", config.writes())
    }
}

#[derive(Debug, Clone, Default)]
pub struct DnfConfBuilder(DnfConf);

impl DnfConfBuilder {
    pub fn add_repo(&mut self, id: String, cfg: impl Into<RepoConf>) -> &mut Self {
        self.0.add_repo(id, cfg.into());
        self
    }

    pub fn build(&self) -> DnfConf {
        self.0.clone()
    }
}

#[derive(Debug, Clone)]
pub struct RepoConf {
    base_urls: Vec<Uri>,
    name: Option<String>,
}

impl From<Vec<Uri>> for RepoConf {
    fn from(uris: Vec<Uri>) -> Self {
        Self {
            base_urls: uris,
            name: None,
        }
    }
}

impl From<Uri> for RepoConf {
    fn from(u: Uri) -> Self {
        vec![u].into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder() {
        let dnf_conf = DnfConf::builder()
            .add_repo(
                "foo".into(),
                vec![
                    "https://repo.repo/yum/my/repo".parse().expect("valid Uri"),
                    "https://mirror.repo/yum/my/repo"
                        .parse()
                        .expect("valid Uri"),
                ],
            )
            .build();
        assert_eq!(
            dnf_conf.to_string(),
            r#"[foo]
baseurl=https://repo.repo/yum/my/repo
        https://mirror.repo/yum/my/repo
"#
        );
    }
}