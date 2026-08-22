//! 类型化错误（架构 §6）。公开 Facade 返回 `anyhow::Result`，内部使用 `BtError` 保留错误分类。

/// 统一错误类型。硬规则（规范"报错"项）经此返回 `Err`，不 panic。
#[derive(thiserror::Error, Debug)]
pub enum BtError {
    /// 数据校验失败：重复键、缺列、非法价格 / factor 等
    #[error("数据校验失败: {0}")]
    Validation(String),
    /// 交易日历相关：区间对齐失败、日期格式非法
    #[error("交易日历: {0}")]
    Calendar(String),
    /// 非法参数：deal_price / benchmark / excess_method / limit_threshold 越界等
    #[error("非法参数: {0}")]
    InvalidParam(String),
    /// 基准数据未覆盖回测区间全部交易日
    #[error("基准覆盖不足: {0}")]
    BenchmarkCoverage(String),
    /// 策略决策非法：同股买卖冲突等
    #[error("决策非法: {0}")]
    InvalidDecision(String),
    #[error(transparent)]
    Polars(#[from] polars::error::PolarsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 内部 Result 别名。
pub type Result<T> = std::result::Result<T, BtError>;
