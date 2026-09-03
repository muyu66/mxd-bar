//! 截图 + 图像分析 + 采样线程。
//!
//! 生产数据流：`capture`(整窗 BGRA) → `digits`(内嵌 0-9 字形模板分类器读全量整数 EXP) →
//! `sampler`(5s 循环、按真实时间戳差分速率、升级/误读判定、写 Shared)。三条都在 release 编译。
//!
//! 反外挂合规不变：只对整窗截图做像素/图像分析（capture → digits），
//! 不读内存、不读色块定位、不实时读屏。

pub mod capture;
pub mod digits;
pub mod sampler;
