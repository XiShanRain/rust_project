//! FR-SRC 数据源模块。每个 provider 一个子模块，统一产出「可搜索的条目快照」。
//!
//! 现阶段落地了六个：FR-SRC-02（应用程序源）、FR-SRC-05（自定义命令源）、
//! FR-SRC-06（路径四态校验）、FR-SRC-10（计算器）、FR-SRC-11（网页搜索）、
//! FR-SRC-14（文件搜索与目录浏览）。
//! 后续 provider（剪贴板历史…）在此目录下并列，届时再抽 `DataSource` trait（FR-SRC-01）。

pub mod application;
pub mod calc;
pub mod custom;
pub mod filesearch;
pub mod icon;
pub mod pathcheck;
pub mod websearch;
