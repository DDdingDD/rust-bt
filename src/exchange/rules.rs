//! 撮合规则纯函数（架构 §4.7，决策 D8）：滑点、费用与 min_cost 反解、整手取整、
//! 涨跌停判定。全部无副作用，是单元测试的主要标的。

use crate::types::{is_star_market, Code};

/// 实际滑点比例（规范"fixed_slippage / min_slippage_ratio"）：
/// `max(min_slippage_ratio, fixed_slippage / trade_price)`。
/// 低价股固定滑点占主导，高价股由最小比例兜底。
pub fn slippage_ratio(fixed_slippage: f64, min_slippage_ratio: f64, trade_price: f64) -> f64 {
    min_slippage_ratio.max(fixed_slippage / trade_price)
}

/// 按方向调整成交价：买得更贵、卖得更便宜。
/// 不 clamp 到涨跌停价（触板由 limit_buy / limit_sell 在撮合前拦截）。
pub fn apply_slippage(trade_price: f64, adj_price_ratio: f64, is_buy: bool) -> f64 {
    if is_buy {
        trade_price * (1.0 + adj_price_ratio)
    } else {
        trade_price * (1.0 - adj_price_ratio)
    }
}

/// 单笔交易费用：`max(trade_val × cost_ratio, min_cost)`。
/// 调用方保证成交量 > 0 时才调用（成交量为 0 时费用归零）。
pub fn trade_cost(trade_val: f64, cost_ratio: f64, min_cost: f64) -> f64 {
    (trade_val * cost_ratio).max(min_cost)
}

/// 买单资金约束反解可买股数（规范"min_cost--现金约束反解"）。
///
/// `deal_price` 为滑点调整后的实际成交价。按两个 regime 分别求解并取可行解中的较大者：
/// - 比例费 regime：`shares ≤ cash / (p × (1 + r))`，要求解落在 `shares × p × r ≥ min_cost`；
/// - 固定费 regime：`shares ≤ (cash − min_cost) / p`，要求解落在 `shares × p × r < min_cost`；
/// - 现金连最低费用都不够时返回 0。
pub fn max_buyable_shares(cash: f64, deal_price: f64, cost_ratio: f64, min_cost: f64) -> f64 {
    if deal_price <= 0.0 || cash <= 0.0 {
        return 0.0;
    }
    let mut best = 0.0f64;
    // 比例费 regime
    let s1 = cash / (deal_price * (1.0 + cost_ratio));
    if s1 > 0.0 && s1 * deal_price * cost_ratio >= min_cost {
        best = best.max(s1);
    }
    // 固定费 regime
    let s2 = (cash - min_cost) / deal_price;
    if s2 > 0.0 && s2 * deal_price * cost_ratio < min_cost {
        best = best.max(s2);
    }
    best
}

/// 买入整手取整（规范"整手取整"）：
/// - 主板：向下取整到 100 股整数倍，不足一手（100 股）为 0；
/// - 科创板（SH688xxx / SH689xxx）：向下取整到 1 股，不足 200 股为 0。
///
/// 卖出不取整（允许零股），调用方不应对卖单调用本函数。
pub fn round_buy_lot(shares: f64, code: Code) -> f64 {
    if shares <= 0.0 {
        return 0.0;
    }
    if is_star_market(code) {
        let s = shares.floor();
        if s < 200.0 {
            0.0
        } else {
            s
        }
    } else {
        let s = (shares / 100.0).floor() * 100.0;
        if s < 100.0 {
            0.0
        } else {
            s
        }
    }
}

/// 涨跌停判定（规范"limit_threshold"）。
///
/// `price` 为 deal_price 对应价格列值；`threshold_ratio = limit_threshold / 0.1`。
/// 返回 (limit_buy, limit_sell)。
///
/// 缺失分支由调用方区分处理（`pre_close` 缺失属常态不告警；`high_limit` / `low_limit`
/// 缺失置 false 并 warning）——本函数假定输入均有效。
pub fn limit_flags(
    pre_close: f64,
    high_limit: f64,
    low_limit: f64,
    price: f64,
    threshold_ratio: f64,
) -> (bool, bool) {
    let up_chg = high_limit / pre_close - 1.0;
    let down_chg = low_limit / pre_close - 1.0;
    let change = price / pre_close - 1.0;
    (
        change >= up_chg * threshold_ratio,
        change <= down_chg * threshold_ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slippage_two_regimes() {
        // 低价股：固定滑点占主导（0.01 / 2.0 = 0.5% > 0.14%）
        assert_eq!(slippage_ratio(0.01, 0.0014, 2.0), 0.005);
        // 高价股：最小比例兜底（0.01 / 100 = 0.01% < 0.14%）
        assert_eq!(slippage_ratio(0.01, 0.0014, 100.0), 0.0014);
        // 方向
        assert!((apply_slippage(10.0, 0.001, true) - 10.01).abs() < 1e-12);
        assert!((apply_slippage(10.0, 0.001, false) - 9.99).abs() < 1e-12);
    }

    #[test]
    fn cost_with_min() {
        assert_eq!(trade_cost(1_000_000.0, 0.00015, 5.0), 150.0); // 比例费
        assert_eq!(trade_cost(1_000.0, 0.00015, 5.0), 5.0); // min_cost 兜底
    }

    #[test]
    fn buyable_proportional_regime() {
        // 大额现金：比例费 regime。cash=100100, p=10, r=0.001, min=5
        // s1 = 100100 / 10.01 = 10000；费 = 10000*10*0.001 = 100 >= 5 可行
        let s = max_buyable_shares(100_100.0, 10.0, 0.001, 5.0);
        assert!((s - 10_000.0).abs() < 1e-9);
    }

    #[test]
    fn buyable_fixed_regime() {
        // 小额现金：固定费 regime。cash=105, p=10, r=0.001, min=5
        // s1 = 105/10.01 ≈ 10.49，费 ≈ 0.105 < 5 不可行；
        // s2 = (105-5)/10 = 10，费 = 10*10*0.001 = 0.1 < 5 可行
        let s = max_buyable_shares(105.0, 10.0, 0.001, 5.0);
        assert!((s - 10.0).abs() < 1e-9);
    }

    #[test]
    fn buyable_insufficient_cash() {
        // 现金连 min_cost 都不够
        assert_eq!(max_buyable_shares(4.0, 10.0, 0.001, 5.0), 0.0);
        // 现金恰好够 min_cost 但买不起任何股
        assert_eq!(max_buyable_shares(5.0, 10.0, 0.001, 5.0), 0.0);
    }

    #[test]
    fn round_lot_main_board() {
        assert_eq!(round_buy_lot(599.9, 600_000), 500.0);
        assert_eq!(round_buy_lot(100.0, 600_000), 100.0);
        assert_eq!(round_buy_lot(99.9, 600_000), 0.0); // 不足一手
        assert_eq!(round_buy_lot(0.0, 600_000), 0.0);
    }

    #[test]
    fn round_lot_star_market() {
        // 科创板：200 股起、按 1 股递增
        assert_eq!(round_buy_lot(250.7, 688_981), 250.0);
        assert_eq!(round_buy_lot(200.0, 688_981), 200.0);
        assert_eq!(round_buy_lot(199.9, 689_009), 0.0); // 不足 200 股
        assert_eq!(round_buy_lot(1500.0, 688_001), 1500.0);
    }

    #[test]
    fn limit_flags_10pct() {
        // 10% 板幅，阈值 0.0985 -> 触发线 9.85%
        let r = 0.0985 / 0.1;
        // 恰好涨停：change = 0.1 >= 0.1*0.985 = 0.0985 -> limit_buy
        assert_eq!(limit_flags(10.0, 11.0, 9.0, 11.0, r), (true, false));
        // 恰好跌停
        assert_eq!(limit_flags(10.0, 11.0, 9.0, 9.0, r), (false, true));
        // 涨 9.8% 未触板
        assert_eq!(limit_flags(10.0, 11.0, 9.0, 10.98, r), (false, false));
        // 涨 9.9% 触板（超过 9.85% 触发线，距涨停 0.1 个百分点）
        assert_eq!(limit_flags(10.0, 11.0, 9.0, 10.99, r), (true, false));
    }

    #[test]
    fn limit_flags_20pct_and_st() {
        let r = 0.0985 / 0.1;
        // 20% 板幅（科创板/创业板）：触发线 19.7%
        assert_eq!(limit_flags(10.0, 12.0, 8.0, 11.98, r), (true, false));
        assert_eq!(limit_flags(10.0, 12.0, 8.0, 11.96, r), (false, false));
        // 5% 板幅（ST）：触发线 4.925%
        assert_eq!(limit_flags(10.0, 10.5, 9.5, 10.5, r), (true, false));
        assert_eq!(limit_flags(10.0, 10.5, 9.5, 10.49, r), (false, false));
    }
}
