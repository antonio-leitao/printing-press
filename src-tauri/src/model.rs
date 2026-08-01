use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub main_file: String,
    pub working_directory: String,
    pub engine: String,
    pub build_status: String,
    pub last_build_at: Option<i64>,
    pub last_build_duration_ms: Option<i64>,
    pub last_error: Option<String>,
    pub artifact_revision: i64,
    pub has_pdf: bool,
    pub path_available: bool,
}

impl ProjectSummary {
    pub fn root(&self) -> PathBuf {
        PathBuf::from(&self.root_path)
    }

    pub fn main_path(&self) -> PathBuf {
        self.root().join(&self.main_file)
    }

    pub fn working_path(&self) -> PathBuf {
        self.root().join(&self.working_directory)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MainCandidate {
    pub relative_path: String,
    pub score: u32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainReport {
    pub latexmk: ToolInfo,
    pub neovim: ToolInfo,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryReport {
    pub root_path: String,
    pub project_name: String,
    pub tex_file_count: usize,
    pub candidates: Vec<MainCandidate>,
    pub recommended_main: Option<String>,
    pub requires_selection: bool,
    pub has_latexmkrc: bool,
    pub warnings: Vec<String>,
    pub toolchain: ToolchainReport,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditorLaunchResult {
    pub status: String,
    pub socket_path: String,
    pub message: String,
}

pub fn path_to_string(path: &Path) -> Option<String> {
    path.to_str().map(ToOwned::to_owned)
}
