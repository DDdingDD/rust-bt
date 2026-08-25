# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

Rust 股票回测系统（A 股日频，暂仅上交所/深交所）。**行为口径的唯一权威是 `doc/specification.md`（设计规范），模块与类型设计依据 `doc/architecture.md`（架构设计）**——修改代码前先对照这两份文档；文档与代码冲突时以规范为准并同步更新文档。

## 常用命令

```bash
cargo build --release                               # 构建
cargo run --release --bin bt -- config.example.yml   # CLI：YAML 配置驱动回测（参数格式见 config.example.yml）
cargo run --release --example run_backtest          # 端到端示例（数据读 tmp_data/，输出写 output/，已 gitignore）
cargo test --lib                                    # 单元测试（rules/types/position/calendar）
cargo test --test acceptance_basic                  # 跑单个验收测试文件
cargo test --test '*'                               # 全部合成验收用例（手算对拍）
cargo test --release --test smoke_tmp_data          # tmp_data 冒烟（加载 937MB CSV，debug 下约 60s，务必用 release）
cargo test --release --test smoke_wap_data          # tmp_data/wap.parquet 时段价冒烟（约 2.7GB，务必用 release）
cargo test                                          # 全量（含 debug 冒烟，慢）
```

## 架构要点（跨文件才能看清的部分）

**权威设计在文档里**：`doc/architecture.md` §4 定义了全部核心类型签名，§5 记录了 14 条关键决策（D1-D14）及其理由。改动撮合、复权、信息边界、WAP 时段价相关代码前必读。

**双层公开 API（决策 D13）**：嵌入外部 Rust 代码用 `api::run`（`BtParams` 类型化参数 -> `BtOutput` 内存结果，可选 `export_all` 导出）；CLI 与示例共用该路径（`BtConfig::to_params`），勿再手写装配。组件 Facade（`BTData`/`Account`/`Exchange`/`Backtest`）供细粒度编排，两层必须共用同一撮合与估值路径（`tests/embedding_api.rs` 对拍守护）。

**主循环时序**（`backtest.rs`，正确性核心）：每个交易日按 `复权调整(adjust_factor) → 取 T−1 日信号 → gen_decision → 阶段一卖单全部撮合 → revise_buy_orders 核减 → 阶段二买单撮合 → end_of_day 估值` 执行。顺序不可调换：复权必须先于撮合（除权日卖单按送转后 volume）；核减必须先卖后买（卖不掉的继续占 top_n 坑）。

**信息边界编译期化**（防前视，决策 D4）：`Signal` 加载时剥离 `ret` 列（结构上不存在）；策略只能拿到 `StrategyContext` 的字段（T−1 信号、持仓、现金、当日 `TradableInfo`）。新增策略可见信息时必须评估是否构成前视。

**双层数据表示**（决策 D2）：polars 只做 IO/校验/报表；主循环用按 (DayIdx, Code) 排序的 SoA `Vec` + `day_offsets` 切片 + 二分查找，禁止逐行访问 DataFrame。`Exchange` 撮合与 `Strategy` 决策共用同一份 `DayView`/`TradableInfo`（口径一致）。

**内部主键**（决策 D6）：字符串 `instrument`（`SH600000`）只在 IO 边界，内部全程 `code: u32`（600000）；日期同理（字符串 ↔ `DayIdx` ↔ `NaiveDate`）。导出时统一反映射。

**判定顺序**：先停牌后涨跌停（停牌行 limit 预计算无意义）；`limit_buy/limit_sell` 在行情注入 Exchange 时按 `deal_price` 列预计算，不能脱离 deal_price 单独算。

**费用口径**：费用直接扣现金、不摊入 `cost_price`；不含成本口径由 `V + 累计费用` 在 Report 层派生（近似，决策 D5）。现金允许为负（卖出费用超成交金额）。

**首日口径**：`r_0 = 0`，首日盈亏只进 `account` 列、不进收益率/净值序列；turnover/cost 分母为前一交易日总资产，首个交易日用期初资金。

**WAP 时段价（决策 D14）**：`deal_price` 支持 `vwapN`/`twapN`（N=1..=11，11 个固定日内窗口的时段表见规范"数据文件格式--wap 数据"），需经 `BTData::load_wap` / `BtParams.wap` / YAML `data.wap` 提供对应窗口的 wap 数据。策略可见价仍为 `pre_close`（防前视），撮合时交易所按方向取 `_buy`/`_sell` 列作为成交价，量上限按方向 `buy_volume`/`sell_volume` 计算；`limit_buy`/`limit_sell` 同样按方向 wap 价对 `pre_close` 预计算。

## 测试约定

- **合成用例是正确性基准**（`tests/acceptance_*.rs`）：价格取整数/有限小数使成交明细与逐日账户可手算对拍，断言完全相等。改撮合/复权/估值逻辑后必须跑全量验收。
- `tests/common/mod.rs` 提供 CSV 合成与对拍辅助；新用例优先复用。
- **`tmp_data/` 仅作格式与规模参考**：不得用其内容（含 hist_position.csv、report_data.csv 样本）验证数值正确性或反推回测参数；冒烟测试不做数值对拍。
- 单元测试集中在 `exchange/rules.rs`（纯函数）与 `types.rs`/`position.rs`/`data/calendar.rs`。
