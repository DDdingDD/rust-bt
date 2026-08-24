//! 交互式 HTML 报告渲染（架构 D10，2026-08 起取代 plotters PNG）：
//! 顶部衍生指标表 + 7 面板堆叠图（累计收益 / 两口径回撤 / 累计超额 / 换手率 / 两口径超额回撤）。
//! plotly.js basic bundle（仅 scatter/bar/pie，覆盖折线图）vendor 在 `assets/` 并内嵌进 HTML，
//! 产物单文件自包含、离线可打开；手工拼 JSON（日期定长、数值最短往返格式），不新增 Rust 依赖。

use chrono::NaiveDate;

use super::DerivedStats;

/// plotly.js basic bundle v2.35.2（MIT，与参考样例 output/report-example.html 同版本）；
/// 内嵌使报告离线可用。文件已确认不含 `</script` / `<!--` 序列，可安全内联。
const PLOTLY_JS: &str = include_str!("../../assets/plotly-basic-2.35.2.min.js");

/// 报告绘图序列（净值口径均为累计净值，渲染时 −1 转为累计收益；回撤为正值，渲染时取负）。
pub(crate) struct ReportCurves<'a> {
    pub dates: &'a [NaiveDate],
    /// 基准累计净值（期初 1）
    pub cum_bench: &'a [f64],
    /// 不含成本累计净值
    pub cum_wo_cost: &'a [f64],
    /// 含成本累计净值
    pub cum_w_cost: &'a [f64],
    /// 不含成本回撤（正值）
    pub mdd_wo_cost: &'a [f64],
    /// 含成本回撤（正值）
    pub mdd_w_cost: &'a [f64],
    /// 不含成本口径累计超额净值
    pub cum_ex_wo_cost: &'a [f64],
    /// 含成本口径累计超额净值
    pub cum_ex_w_cost: &'a [f64],
    /// 双边换手率（日度）
    pub turnover: &'a [f64],
    /// 含成本口径超额净值回撤（正值）
    pub ex_mdd_w_cost: &'a [f64],
    /// 不含成本口径超额净值回撤（正值）
    pub ex_mdd_wo_cost: &'a [f64],
}

/// f64 -> JSON 字面量：Display 为最短往返格式；非有限值（NaN/Inf）降级为 null（JSON 无此字面量）。
fn js_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

/// 序列 -> JSON 数组；`offset` 为逐元素加性偏移（净值 −1 转累计收益），`negate` 取负（回撤负值绘制）。
/// 首部统一补 T0 基准点 0。
fn js_series(xs: &[f64], offset: f64, negate: bool) -> String {
    let mut s = String::with_capacity(xs.len() * 10 + 3);
    s.push_str("[0");
    for v in xs {
        let v = if negate { -(v + offset) } else { v + offset };
        s.push(',');
        s.push_str(&js_f64(v));
    }
    s.push(']');
    s
}

/// 日期轴 -> JSON 数组，首部补 "T0"。
fn js_dates(dates: &[NaiveDate]) -> String {
    let mut s = String::with_capacity(dates.len() * 13 + 8);
    s.push_str("[\"T0\"");
    for d in dates {
        s.push_str(",\"");
        s.push_str(&d.format("%Y-%m-%d").to_string());
        s.push('\"');
    }
    s.push(']');
    s
}

/// 7 面板 y 轴 domain（比例与参考样例主图一致：累计收益 / 累计超额为高面板，间隙 0.01）。
const Y_DOMAINS: [(f64, f64); 7] = [
    (0.7436, 1.0),
    (0.6482, 0.7336),
    (0.5527, 0.6382),
    (0.2864, 0.5427),
    (0.1909, 0.2764),
    (0.0955, 0.1809),
    (0.0, 0.0855),
];

/// 子图标题（中文，annotation 标注于各面板左上）。
const PANEL_TITLES: [&str; 7] = [
    "累计收益",
    "回撤（不含成本）",
    "回撤（含成本）",
    "累计超额收益",
    "换手率",
    "超额回撤（含成本）",
    "超额回撤（不含成本）",
];

fn push_trace(out: &mut String, name: &str, x: &str, y: &str, xaxis: &str, yaxis: &str, fill: bool) {
    // lines+markers：悬停单点标记（qlib 样例口径）；回撤类 fill tozeroy 面积填充
    out.push_str("{\"type\":\"scatter\",\"mode\":\"lines+markers\",\"name\":\"");
    out.push_str(name);
    out.push_str("\",\"x\":");
    out.push_str(x);
    out.push_str(",\"y\":");
    out.push_str(y);
    if fill {
        out.push_str(",\"fill\":\"tozeroy\"");
    }
    out.push_str(",\"xaxis\":\"");
    out.push_str(xaxis);
    out.push_str("\",\"yaxis\":\"");
    out.push_str(yaxis);
    out.push_str("\"}");
}

/// 渲染完整报告 HTML（纯函数，便于单测）。
pub(crate) fn render_html(curves: &ReportCurves, derived: &DerivedStats) -> String {
    let n = curves.dates.len();
    debug_assert!(curves.cum_bench.len() == n && curves.turnover.len() == n);

    let x = js_dates(curves.dates);
    // 净值 −1 -> 累计收益；回撤取负
    let cum_bench = js_series(curves.cum_bench, -1.0, false);
    let cum_wo = js_series(curves.cum_wo_cost, -1.0, false);
    let cum_w = js_series(curves.cum_w_cost, -1.0, false);
    let mdd_wo = js_series(curves.mdd_wo_cost, 0.0, true);
    let mdd_w = js_series(curves.mdd_w_cost, 0.0, true);
    let ex_wo = js_series(curves.cum_ex_wo_cost, -1.0, false);
    let ex_w = js_series(curves.cum_ex_w_cost, -1.0, false);
    let turnover = js_series(curves.turnover, 0.0, false);
    let ex_mdd_w = js_series(curves.ex_mdd_w_cost, 0.0, true);
    let ex_mdd_wo = js_series(curves.ex_mdd_wo_cost, 0.0, true);

    let mut traces = String::with_capacity(64 * 1024);
    traces.push('[');
    let mut first = true;
    let mut t = |name: &str, y: &str, i: usize, fill: bool| {
        if !first {
            traces.push(',');
        }
        first = false;
        let (xaxis, yaxis) = if i == 0 {
            ("x".to_string(), "y".to_string())
        } else {
            (format!("x{}", i + 1), format!("y{}", i + 1))
        };
        push_trace(&mut traces, name, &x, y, &xaxis, &yaxis, fill);
    };
    t("cum bench", &cum_bench, 0, false);
    t("cum return wo cost", &cum_wo, 0, false);
    t("cum return w cost", &cum_w, 0, false);
    t("return wo mdd", &mdd_wo, 1, true);
    t("return w cost mdd", &mdd_w, 2, true);
    t("cum ex return wo cost", &ex_wo, 3, false);
    t("cum ex return w cost", &ex_w, 3, false);
    t("turnover", &turnover, 4, false);
    t("cum ex return w cost mdd", &ex_mdd_w, 5, true);
    t("cum ex return wo cost mdd", &ex_mdd_wo, 6, true);
    traces.push(']');

    // 布局：7 对 x/y 轴，与 qlib 样例口径一致 —— x 轴 category + 45° 刻度，
    // 上 6 轴 matches 最底轴 x7（缩放联动），y 轴 zeroline/showline 开启
    let mut layout = String::with_capacity(4096);
    layout.push_str("{\"height\":1200,\"title\":{\"text\":\" \"},");
    for (i, (lo, hi)) in Y_DOMAINS.iter().enumerate() {
        let suffix = if i == 0 { String::new() } else { (i + 1).to_string() };
        layout.push_str(&format!(
            "\"yaxis{suffix}\":{{\"domain\":[{lo},{hi}],\"anchor\":\"x{suffix}\",\"zeroline\":true,\"showline\":true,\"showticklabels\":true}},"
        ));
        if i + 1 == Y_DOMAINS.len() {
            layout.push_str(&format!(
                "\"xaxis{suffix}\":{{\"anchor\":\"y{suffix}\",\"domain\":[0.0,1.0],\"showline\":true,\"type\":\"category\",\"tickangle\":45}},"
            ));
        } else {
            layout.push_str(&format!(
                "\"xaxis{suffix}\":{{\"anchor\":\"y{suffix}\",\"domain\":[0.0,1.0],\"matches\":\"x7\",\"showticklabels\":false,\"showline\":false,\"type\":\"category\",\"tickangle\":45}},"
            ));
        }
    }
    layout.push_str("\"annotations\":[");
    for (i, title) in PANEL_TITLES.iter().enumerate() {
        if i > 0 {
            layout.push(',');
        }
        layout.push_str(&format!(
            "{{\"text\":\"{title}\",\"xref\":\"paper\",\"yref\":\"paper\",\"x\":0,\"y\":{},\"showarrow\":false,\"xanchor\":\"left\",\"yanchor\":\"bottom\",\"font\":{{\"size\":13}}}}",
            Y_DOMAINS[i].1
        ));
    }
    layout.push_str("]}");

    let mut html = String::with_capacity(PLOTLY_JS.len() + traces.len() + 8192);
    // meta charset：页面含中文指标表与子图标题，本地打开时确保按 UTF-8 解析
    html.push_str("<meta charset=\"utf-8\">\n<div></div><div>\n");
    html.push_str("<script type=\"text/javascript\">window.PlotlyConfig = {MathJaxConfig: 'local'};</script>\n");
    // 内嵌 plotly.js（basic bundle），报告单文件自包含、离线可打开
    html.push_str("<script charset=\"utf-8\">\n");
    html.push_str(PLOTLY_JS);
    html.push_str("\n</script>\n");
    html.push_str(&metrics_table(derived));
    html.push_str("<div id=\"report-plot\" class=\"plotly-graph-div\" style=\"height:1200px; width:100%;\"></div>\n");
    html.push_str("<script type=\"text/javascript\">\nPlotly.newPlot(\"report-plot\",");
    html.push_str(&traces);
    html.push(',');
    html.push_str(&layout);
    html.push_str(",{\"responsive\":true});\n</script>\n</div>\n");
    html
}

/// 衍生指标表（含 / 不含成本两列口径；波动率、夏普、最大回撤仅含成本口径）。
fn metrics_table(d: &DerivedStats) -> String {
    let pct = |v: f64| format!("{:.2}%", v * 100.0);
    let num = |v: f64| format!("{v:.4}");
    let row = |name: &str, w: String, wo: &str| {
        format!("<tr><td>{name}</td><td>{w}</td><td>{wo}</td></tr>\n")
    };
    let mut s = String::with_capacity(2048);
    s.push_str("<h3>风险指标</h3>\n<table border=\"1\" cellspacing=\"0\" cellpadding=\"4\" style=\"border-collapse:collapse;\">\n");
    s.push_str("<tr><th>指标</th><th>含成本</th><th>不含成本</th></tr>\n");
    s.push_str(&row("年化收益率", pct(d.annualized_return), &pct(d.annualized_return_without_cost)));
    s.push_str(&row("年化波动率", pct(d.annualized_volatility), "—"));
    s.push_str(&row("夏普比率", num(d.sharpe), "—"));
    s.push_str(&row("最大回撤", pct(d.max_drawdown), "—"));
    s.push_str(&row("超额年化收益率", pct(d.excess_annualized_return), &pct(d.excess_annualized_return_without_cost)));
    s.push_str(&row("信息比率", num(d.information_ratio), &num(d.information_ratio_without_cost)));
    s.push_str("</table>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_curves(dates: &[NaiveDate]) -> ReportCurves<'_> {
        // 与 dates 等长的常量序列（净值 1.0/回撤 0/换手 0）
        static ZEROS: [f64; 3] = [0.0; 3];
        static ONES: [f64; 3] = [1.0; 3];
        let _ = dates;
        ReportCurves {
            dates,
            cum_bench: &ONES,
            cum_wo_cost: &ONES,
            cum_w_cost: &ONES,
            mdd_wo_cost: &ZEROS,
            mdd_w_cost: &ZEROS,
            cum_ex_wo_cost: &ONES,
            cum_ex_w_cost: &ONES,
            turnover: &ZEROS,
            ex_mdd_w_cost: &ZEROS,
            ex_mdd_wo_cost: &ZEROS,
        }
    }

    fn three_dates() -> Vec<NaiveDate> {
        vec![
            NaiveDate::from_ymd_opt(2023, 1, 3).unwrap(),
            NaiveDate::from_ymd_opt(2023, 1, 4).unwrap(),
            NaiveDate::from_ymd_opt(2023, 1, 5).unwrap(),
        ]
    }

    #[test]
    fn embeds_plotly_inline_and_plot_call() {
        let dates = three_dates();
        let html = render_html(&sample_curves(&dates), &DerivedStats::default());
        // plotly.js 内嵌（离线可用），不再以 src 引用 CDN
        assert!(html.contains("plotly.js (basic - minified) v2.35.2"));
        assert!(!html.contains("src=\"https://cdn.plot.ly"));
        assert!(html.contains("风险指标"));
        assert!(html.contains("Plotly.newPlot(\"report-plot\""));
        // T0 基准点 + 全部日期
        assert!(html.contains("[\"T0\",\"2023-01-03\",\"2023-01-04\",\"2023-01-05\"]"));
    }

    #[test]
    fn has_ten_traces_and_seven_panels() {
        let dates = three_dates();
        let html = render_html(&sample_curves(&dates), &DerivedStats::default());
        for name in [
            "cum bench",
            "cum return wo cost",
            "cum return w cost",
            "return wo mdd",
            "return w cost mdd",
            "cum ex return wo cost",
            "cum ex return w cost",
            "turnover",
            "cum ex return w cost mdd",
            "cum ex return wo cost mdd",
        ] {
            assert!(html.contains(&format!("\"name\":\"{name}\"")), "缺少 trace {name}");
        }
        for i in 2..=7 {
            assert!(html.contains(&format!("\"yaxis{i}\"")), "缺少 yaxis{i}");
        }
        for title in PANEL_TITLES {
            assert!(html.contains(title), "缺少子图标题 {title}");
        }
    }

    #[test]
    fn interaction_matches_qlib_example() {
        let dates = three_dates();
        let html = render_html(&sample_curves(&dates), &DerivedStats::default());
        // 10 条 trace 全部 lines+markers（悬停单点标记）
        assert_eq!(html.matches("\"mode\":\"lines+markers\"").count(), 10);
        // 4 条回撤类 trace 面积填充
        assert_eq!(html.matches("\"fill\":\"tozeroy\"").count(), 4);
        // 7 根 category x 轴 + 45° 刻度，上 6 轴 matches 最底轴
        assert_eq!(html.matches("\"type\":\"category\"").count(), 7);
        assert_eq!(html.matches("\"tickangle\":45").count(), 7);
        assert_eq!(html.matches("\"matches\":\"x7\"").count(), 6);
        // 不启用 x unified（默认 closest 单点提示，与样例一致）；
        // 内嵌 plotly.js 自身含 "hovermode" 字样，只校验生成段（指标表起）
        let data = &html[html.find("<h3>").unwrap()..];
        assert!(!data.contains("hovermode"));
    }

    #[test]
    fn net_value_shifted_and_drawdown_negated() {
        let dates = three_dates();
        let mut curves = sample_curves(&dates);
        let cum = [1.0, 1.1, 0.99];
        let dd = [0.0, 0.05, 0.1];
        curves.cum_w_cost = &cum;
        curves.mdd_w_cost = &dd;
        let html = render_html(&curves, &DerivedStats::default());
        // 累计收益：T0=0，净值 −1
        assert!(html.contains("[0,0,0.10000000000000009,-0.010000000000000009]"));
        // 回撤取负
        assert!(html.contains("[0,-0,-0.05,-0.1]"));
    }

    #[test]
    fn non_finite_values_become_null() {
        let dates = three_dates();
        let mut curves = sample_curves(&dates);
        let bad = [1.0, f64::NAN, f64::INFINITY];
        curves.cum_bench = &bad;
        let html = render_html(&curves, &DerivedStats::default());
        assert!(html.contains("[0,0,null,null]"));
        // 内嵌的 plotly.js 自身含 "NaN"/"inf" 字样，只校验生成的数据段（指标表起）
        let data = &html[html.find("<h3>").unwrap()..];
        assert!(!data.contains("NaN"));
        assert!(!data.contains("inf"));
    }

    #[test]
    fn metrics_table_values() {
        let dates = three_dates();
        let d = DerivedStats {
            annualized_return: 0.1234,
            annualized_volatility: 0.2,
            sharpe: 1.5,
            max_drawdown: 0.05,
            excess_annualized_return: 0.01,
            information_ratio: 0.5,
            annualized_return_without_cost: 0.15,
            excess_annualized_return_without_cost: 0.03,
            information_ratio_without_cost: 0.8,
        };
        let html = render_html(&sample_curves(&dates), &d);
        assert!(html.contains("<td>年化收益率</td><td>12.34%</td><td>15.00%</td>"));
        assert!(html.contains("<td>夏普比率</td><td>1.5000</td><td>—</td>"));
        assert!(html.contains("<td>信息比率</td><td>0.5000</td><td>0.8000</td>"));
    }
}
