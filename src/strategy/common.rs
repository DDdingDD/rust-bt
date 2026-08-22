//! 策略可复用构件（架构 §4.6）：排名（同分字典序确定性）、等权资金分配、金额->股数换算。

use crate::types::Code;

/// 按 score 降序排名；同分按 code 升序（买入排名：同分代码小者优先）。
pub fn sort_by_score_desc(items: &mut [(Code, f64)]) {
    items.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

/// 按 score 升序排名；同分按 code 升序（卖出排名：同分代码小者优先）。
pub fn sort_by_score_asc(items: &mut [(Code, f64)]) {
    items.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

/// 等权资金分配：总额 / 只数。n == 0 时返回 None。
pub fn equal_weight_amount(total: f64, n: usize) -> Option<f64> {
    if n == 0 {
        None
    } else {
        Some(total / n as f64)
    }
}

/// 金额 -> 委托股数换算（最终以 Exchange 撮合与整手取整为准）。
pub fn amount_to_volume(amount: f64, price: f64) -> f64 {
    if price <= 0.0 || price.is_nan() {
        0.0
    } else {
        amount / price
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tie_break_by_code() {
        // 同分确定性：买入降序同分代码小者优先，卖出升序同分代码小者优先
        let mut buy = vec![(300_750, 1.0), (600_000, 1.0), (6, 2.0)];
        sort_by_score_desc(&mut buy);
        assert_eq!(buy, vec![(6, 2.0), (300_750, 1.0), (600_000, 1.0)]);

        let mut sell = vec![(600_000, 1.0), (300_750, 1.0), (6, 0.5)];
        sort_by_score_asc(&mut sell);
        assert_eq!(sell, vec![(6, 0.5), (300_750, 1.0), (600_000, 1.0)]);
    }
}
