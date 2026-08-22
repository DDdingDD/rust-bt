# rust-bt

Rust 股票回测系统（A 股日频）。设计依据：

- `doc/specification.md` —— 设计规范（行为口径的唯一权威）
- `doc/architecture.md` —— 系统架构（模块划分、类型设计、关键决策）

## 构建与运行

```bash
cargo build --release
# 端到端示例（数据路径指向 tmp_data/，可在 examples/run_backtest.rs 中修改）
cargo run --release --example run_backtest
```

输出统一写入 `output/`（已 gitignore）：`hist_position.csv`、`trades.csv`、`report_data.csv`、`report_plot.png`。

## 测试

```bash
cargo test --lib                                    # 单元测试（rules/types/position/calendar）
cargo test --test '*'                               # 合成用例精确验收（手算对拍）
cargo test --release --test smoke_tmp_data          # tmp_data 端到端冒烟（数据缺失自动跳过）
```

## 用法

见 `examples/run_backtest.rs`，与规范"使用方法"一致：加载信号与行情 →
配置 Exchange（成交价 / 费率 / 滑点 / 成交量与涨跌停约束）→
`TopkDropoutStrategy`（或自定义 `Strategy` trait 实现）→ `Backtest::run` →
导出持仓 / 成交 / 报表。
