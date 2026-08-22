//! Order / Decision / TradeRecord（架构 §4.4）。

use crate::types::{Code, DayIdx};

/// 订单（规范"核心概念--Order"）。
#[derive(Clone, Debug)]
pub struct Order {
    /// 股票代码（int 编码）
    pub stock: Code,
    /// 委托数量（股，买为正、卖为负）
    pub volume: f64,
    /// 委托价格（策略生成时填写，内置策略取 T_exec 日 deal_price 列）
    pub price: f64,
    /// 实际成交数量（带符号），由 Exchange 成交后回填
    pub deal_volume: f64,
    /// 实际成交价格（含滑点），由 Exchange 成交后回填
    pub deal_price: f64,
    /// 实际交易费用，由 Exchange 成交后回填
    pub deal_cost: f64,
}

impl Order {
    pub fn new(stock: Code, volume: f64, price: f64) -> Self {
        Self {
            stock,
            volume,
            price,
            deal_volume: 0.0,
            deal_price: 0.0,
            deal_cost: 0.0,
        }
    }

    /// 是否买单（volume > 0）。
    pub fn is_buy(&self) -> bool {
        self.volume > 0.0
    }
}

/// 策略单步输出（规范"核心概念--Decision"）。
///
/// 分卖 / 买两组是规范"先卖后买"与两阶段撮合的直接表达。
#[derive(Clone, Debug, Default)]
pub struct Decision {
    pub sell_orders: Vec<Order>,
    /// 按优先级降序排列（如 score 降序，分数高的优先获得资金；核减时从尾部丢弃）
    pub buy_orders: Vec<Order>,
    /// 默认核减钩子（`Strategy::revise_buy_orders`）的输入：卖出成交后，
    /// 买单只数核减至 target − 实际持仓数。`None` 表示默认不核减。
    pub target_positions: Option<usize>,
}

/// 买卖方向（trades.csv `side` 列）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// 成交记录（trades.csv 行）：Order 的导出投影 + datetime / side。
/// volume / deal_volume 存绝对值。
#[derive(Clone, Debug)]
pub struct TradeRecord {
    pub day: DayIdx,
    pub stock: Code,
    pub side: Side,
    /// 委托数量（绝对值）
    pub volume: f64,
    /// 委托价格
    pub price: f64,
    /// 实际成交数量（绝对值，未成交为 0）
    pub deal_volume: f64,
    /// 实际成交价（含滑点）
    pub deal_price: f64,
    /// 实际交易费用
    pub deal_cost: f64,
}
