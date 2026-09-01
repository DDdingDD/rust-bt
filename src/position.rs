//! Position（持仓，架构 §4.5）：持仓项与 factor 复权调整。

use crate::types::Code;

/// factor 比较的相对 epsilon（规范"复权处理"：判定 factor_today ≠ last_factor 带 epsilon）。
pub const FACTOR_EPS: f64 = 1e-12;

/// 持仓项（规范"核心概念--Position"）。
///
/// `cost_price` 与 `price` 必须同时存在：前者记账算盈亏，后者每日估值。
#[derive(Clone, Copy, Debug)]
pub struct PositionEntry {
    /// 持有数量（f64 存储，送转除权可为非整数）
    pub volume: f64,
    /// 持仓成本价（复权调整后的当前成本，费用不摊入）
    pub cost_price: f64,
    /// 最新有效收盘价（估值用；停牌 / 退市沿用最近有效值）
    pub price: f64,
    /// 最近一次入账时的复权因子
    pub last_factor: f64,
    /// 连续持仓天数（买入成交日记 1，每持有一个交易日 +1）
    pub count_day: u32,
}

impl PositionEntry {
    /// 持仓市值。
    pub fn market_value(&self) -> f64 {
        self.volume * self.price
    }
}

/// 单只持仓的 factor 复权调整（规范"复权处理"）。
///
/// `factor_today` 为当日行情 factor；与持仓自身 `last_factor` 不同（epsilon 比较）时：
/// `volume ×= factor_today / last_factor`，`cost_price /= ratio`，`price /= ratio`，
/// 并更新 `last_factor`。`price` 同步调整是为了保证：在当日无有效收盘价（停牌行）
/// 的除权日，`end_of_day` 不会刷新 price，市值仍按复权后价格连续；有有效收盘价时
/// `end_of_day` 会覆盖 price，因此不影响正常交易日。
/// 返回是否发生了调整。
pub fn adjust_entry_factor(entry: &mut PositionEntry, factor_today: f64) -> bool {
    let ratio = factor_today / entry.last_factor;
    if (ratio - 1.0).abs() <= FACTOR_EPS {
        return false;
    }
    entry.volume *= ratio;
    entry.cost_price /= ratio;
    entry.price /= ratio;
    entry.last_factor = factor_today;
    true
}

/// 持仓容器：BTreeMap 保证遍历有序（逐日估值求和确定性）。
pub type Positions = std::collections::BTreeMap<Code, PositionEntry>;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> PositionEntry {
        PositionEntry {
            volume: 1000.0,
            cost_price: 20.0,
            price: 20.0,
            last_factor: 1.0,
            count_day: 3,
        }
    }

    #[test]
    fn factor_double() {
        // 10 送 10：factor 1 -> 2，股数翻倍、成本减半、市值连续
        let mut e = entry();
        assert!(adjust_entry_factor(&mut e, 2.0));
        assert_eq!(e.volume, 2000.0);
        assert_eq!(e.cost_price, 10.0);
        assert_eq!(e.last_factor, 2.0);
        // 再次相同 factor 不再调整
        assert!(!adjust_entry_factor(&mut e, 2.0));
    }

    #[test]
    fn factor_fractional() {
        // 10 送 3.5：股数可为非整数
        let mut e = entry();
        assert!(adjust_entry_factor(&mut e, 1.35));
        assert!((e.volume - 1350.0).abs() < 1e-9);
        assert!((e.cost_price - 20.0 / 1.35).abs() < 1e-9);
    }

    #[test]
    fn epsilon_comparison() {
        let mut e = entry();
        // 相对变化在 epsilon 内不调整
        assert!(!adjust_entry_factor(&mut e, 1.0 * (1.0 + 1e-13)));
        assert_eq!(e.volume, 1000.0);
    }

    #[test]
    fn resume_after_suspend() {
        // 停牌期间 factor 变化，恢复后一次性补调（以持仓自身 last_factor 为基准）
        let mut e = entry();
        // 停牌数日无行情，恢复当日 factor = 4.0（累计两次除权 2×2）
        assert!(adjust_entry_factor(&mut e, 4.0));
        assert_eq!(e.volume, 4000.0);
        assert_eq!(e.cost_price, 5.0);
    }
}
