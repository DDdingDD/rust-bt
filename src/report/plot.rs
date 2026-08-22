//! plotters 绘图（架构 D10）：净值 / 回撤 / 超额三子图 -> report_plot.png。

use chrono::NaiveDate;
use plotters::prelude::*;

use crate::error::{BtError, Result};

/// 绘制报告 PNG：三个纵向子图，X 轴为交易日。
pub(crate) fn plot_report(
    path: &str,
    dates: &[NaiveDate],
    cum_with_cost: &[f64],
    cum_benchmark: &[f64],
    drawdown: &[f64],
    cum_excess: &[f64],
) -> Result<()> {
    if dates.is_empty() {
        return Err(BtError::Validation("无回测数据，无法绘图".into()));
    }
    let n = dates.len();

    let root = BitMapBackend::new(path, (1200, 900)).into_drawing_area();
    root.fill(&WHITE)
        .map_err(|e| BtError::Validation(format!("绘图失败: {e}")))?;
    let areas = root.split_evenly((3, 1));

    let y_range = |series: &[f64]| -> (f64, f64) {
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        for v in series {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        if lo >= hi {
            lo -= 0.01;
            hi += 0.01;
        } else {
            let pad = (hi - lo) * 0.05;
            lo -= pad;
            hi += pad;
        }
        (lo, hi)
    };

    // X 轴标签：交易日索引 -> 日期字符串（稀疏标注由 plotters 自动处理）
    let x_labels: Vec<String> = dates
        .iter()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect();

    let draw_panel = |area: &DrawingArea<BitMapBackend, plotters::coord::Shift>,
                          title: &str,
                          lines: &[(&str, &[f64], RGBColor)]|
     -> Result<()> {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for (_, s, _) in lines {
            let (l, h) = y_range(s);
            lo = lo.min(l);
            hi = hi.max(h);
        }
        let mut chart = ChartBuilder::on(area)
            .caption(title, ("sans-serif", 20))
            .margin(10)
            .x_label_area_size(30)
            .y_label_area_size(60)
            .build_cartesian_2d(0..(n - 1), lo..hi)
            .map_err(|e| BtError::Validation(format!("绘图失败: {e}")))?;
        chart
            .configure_mesh()
            .x_label_formatter(&|x| {
                let i = (*x as usize).min(n - 1);
                x_labels[i].clone()
            })
            .x_labels(8)
            .draw()
            .map_err(|e| BtError::Validation(format!("绘图失败: {e}")))?;
        for (name, series, color) in lines {
            chart
                .draw_series(LineSeries::new(
                    series.iter().enumerate().map(|(i, v)| (i, *v)),
                    color.stroke_width(1),
                ))
                .map_err(|e| BtError::Validation(format!("绘图失败: {e}")))?
                .label(*name)
                .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color.stroke_width(2)));
        }
        chart
            .configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .border_style(BLACK)
            .draw()
            .map_err(|e| BtError::Validation(format!("绘图失败: {e}")))?;
        Ok(())
    };

    draw_panel(
        &areas[0],
        "净值",
        &[
            ("strategy", cum_with_cost, BLUE),
            ("benchmark", cum_benchmark, RGBColor(160, 160, 160)),
        ],
    )?;
    draw_panel(&areas[1], "回撤", &[("drawdown", drawdown, RED)])?;
    draw_panel(&areas[2], "超额（累计）", &[("excess", cum_excess, GREEN)])?;

    root.present()
        .map_err(|e| BtError::Validation(format!("绘图输出失败: {e}")))?;
    Ok(())
}
