//! FR-SRC 数据源模块。每个 provider 一个子模块，统一产出「可搜索的条目快照」。
//!
//! 现阶段落地了三个：FR-SRC-02（应用程序源）、FR-SRC-10（计算器）、FR-SRC-11（网页搜索）。
//! 后续 provider（文件搜索、剪贴板历史…）在此目录下并列，届时再抽 `DataSource` trait（FR-SRC-01）。

pub mod application;
pub mod calc;
pub mod icon;
pub mod websearch;
