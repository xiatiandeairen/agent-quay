//! app：用例编排。固定节奏：开事务 → 读快照 → core 算出动作 → 执行动作 → 提交。

pub mod check;
pub mod ctx;
pub mod error;
pub mod estimate;
pub mod hold;
pub mod human;
pub mod logs;
pub mod monitor;
pub mod release;
pub mod run;
pub mod runner;
pub mod simgc;
pub mod spawn;
pub mod status;
pub mod top;
pub mod usage;
pub mod view;
