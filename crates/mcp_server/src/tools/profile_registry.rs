//! MCP tools for VoidCrawl-managed Chromium profile metadata and pools.

#![allow(clippy::unused_async)]

use rmcp::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{
    ManagedProfileDescription, ProfilePool, ProfileRegistry, ResolvedProfilePool,
};

use crate::{errors::map_err, server::VoidCrawlServer};

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfileListArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileListResult {
    pub profiles: Vec<ManagedProfileDescription>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfileCreateArgs {
    pub id:          String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels:      Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfileDescribeArgs {
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfileCloneArgs {
    pub source_id_or_path: String,
    pub id:                String,
    #[serde(default)]
    pub description:       Option<String>,
    #[serde(default)]
    pub labels:            Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfileDeleteArgs {
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileDeleteResult {
    pub deleted: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfilePoolListArgs {}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfilePoolListResult {
    pub pools: Vec<ProfilePool>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfilePoolCreateArgs {
    pub name:        String,
    pub profile_ids: Vec<String>,
    #[serde(default = "default_max_active")]
    pub max_active:  usize,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct ProfilePoolDescribeArgs {
    pub name: String,
}

fn default_max_active() -> usize {
    3
}

pub async fn list(_: &VoidCrawlServer, _: ProfileListArgs) -> Result<ProfileListResult, ErrorData> {
    ProfileRegistry::default()
        .list_profiles()
        .map(|profiles| ProfileListResult { profiles })
        .map_err(map_err)
}

pub async fn create(
    _: &VoidCrawlServer,
    args: ProfileCreateArgs,
) -> Result<ManagedProfileDescription, ErrorData> {
    ProfileRegistry::default()
        .create_profile(&args.id, args.description, args.labels)
        .map_err(map_err)
}

pub async fn describe(
    _: &VoidCrawlServer,
    args: ProfileDescribeArgs,
) -> Result<ManagedProfileDescription, ErrorData> {
    ProfileRegistry::default().describe_profile(&args.id).map_err(map_err)
}

pub async fn clone(
    _: &VoidCrawlServer,
    args: ProfileCloneArgs,
) -> Result<ManagedProfileDescription, ErrorData> {
    ProfileRegistry::default()
        .clone_profile(&args.source_id_or_path, &args.id, args.description, args.labels)
        .map_err(map_err)
}

pub async fn delete(
    _: &VoidCrawlServer,
    args: ProfileDeleteArgs,
) -> Result<ProfileDeleteResult, ErrorData> {
    ProfileRegistry::default()
        .delete_profile(&args.id)
        .map(|deleted| ProfileDeleteResult { deleted })
        .map_err(map_err)
}

pub async fn pool_list(
    _: &VoidCrawlServer,
    _: ProfilePoolListArgs,
) -> Result<ProfilePoolListResult, ErrorData> {
    ProfileRegistry::default()
        .list_pools()
        .map(|pools| ProfilePoolListResult { pools })
        .map_err(map_err)
}

pub async fn pool_create(
    _: &VoidCrawlServer,
    args: ProfilePoolCreateArgs,
) -> Result<ProfilePool, ErrorData> {
    ProfileRegistry::default()
        .create_pool(&args.name, args.profile_ids, args.max_active)
        .map_err(map_err)
}

pub async fn pool_describe(
    _: &VoidCrawlServer,
    args: ProfilePoolDescribeArgs,
) -> Result<ResolvedProfilePool, ErrorData> {
    ProfileRegistry::default().resolve_pool(&args.name).map_err(map_err)
}
