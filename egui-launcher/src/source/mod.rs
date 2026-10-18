//! FR-SRC 数据源模块。每个 provider 一个子模块，统一产出「可搜索的条目快照」。
//!
//! 现阶段只落地 FR-SRC-02（应用程序源）。后续 provider（计算器、网页搜索、文件搜索…）
//! 在此目录下并列，届时再抽 `DataSource` trait（FR-SRC-01）。

pub mod application;
pub mod icon;
