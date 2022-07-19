/*
 * Copyright (c) Meta Platforms, Inc. and its affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use fbinit::FacebookInit;

use metalos_thrift_host_configs::api::server::Metalctl;
use metalos_thrift_host_configs::api::services::metalctl::OnlineUpdateCommitExn;
use metalos_thrift_host_configs::api::services::metalctl::OnlineUpdateStageExn;
use metalos_thrift_host_configs::api::OnlineUpdateCommitResponse;
use metalos_thrift_host_configs::api::OnlineUpdateRequest;
use metalos_thrift_host_configs::api::UpdateStageResponse;

use async_trait::async_trait;

#[derive(Clone)]
pub struct Metald {
    pub fb: FacebookInit,
}

#[async_trait]
impl Metalctl for Metald {
    async fn online_update_stage(
        &self,
        _input: OnlineUpdateRequest,
    ) -> Result<UpdateStageResponse, OnlineUpdateStageExn> {
        Ok(UpdateStageResponse {
            ..Default::default()
        })
    }

    async fn online_update_commit(
        &self,
        _input: OnlineUpdateRequest,
    ) -> Result<OnlineUpdateCommitResponse, OnlineUpdateCommitExn> {
        Ok(OnlineUpdateCommitResponse {
            ..Default::default()
        })
    }
}