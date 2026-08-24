# rust-bt 嵌入 API 文档

本文面向**把回测引擎嵌入其他 Rust 代码**的调用方：一次 `run` 调用完成装配、
回测与报告，参数类型化、结果直接在内存中消费，可选导出文件产物。

> 行为口径的唯一权威是 [`doc/specification.md`](specification.md)（设计规范）；
> 本文是其嵌入视角的使用文档，类型与模块设计见 [`doc/architecture.md`](architecture.md) §4.10 / D13。
> 数据文件格式（stock_bar / benchmark / pred CSV）的字段定义见规范"数据文件格式"一节。

---

## 目录

1. [引入依赖](#1-引入依赖)
2. [30 秒上手](#2-30-秒上手)
3. [接口总览](#3-接口总览)
4. [参数详解 BtParams](#4-参数详解-btparams)
5. [信号：文件加载与内存构造](#5-信号文件加载与内存构造)
6. [结果消费 BtOutput](#6-结果消费-btoutput)
7. [文件导出](#7-文件导出)
8. [自定义策略](#8-自定义策略)
9. [错误处理](#9-错误处理)
10. [调用示例集](#10-调用示例集)
11. [注意事项](#11-注意事项)

---

## 1. 引入依赖

包名 `rust-bt`（库名 `rust_bt`），本机 path 或私有 git 引入：

```toml
[dependencies]
rust-bt = { path = "../rust-bt" }
# 或私有 git
# rust-bt = { git = "ssh://git@example.com/quant/rust-bt.git", branch = "main" }
```

可选：初始化一个 `log` 实现以接收告警（信号丢弃、卖出截断等），否则告警被静默丢弃：

```rust
env_logger::init(); // 任意 log facade 实现均可
```

## 2. 30 秒上手

```rust
use rust_bt::{run_from_signal_file, BtParams, ExchangeParams, StrategySpec};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let params = BtParams {
        stock_bar: "tmp_data/stock_bar.csv".into(),   // 股票日行情（交易日历来源）
        benchmark: "tmp_data/benchmark.csv".into(),   // 基准收益（报告必需）
        start_date: "2026-01-01".into(),              // 区间 [start, end)，按交易日历自动对齐
        end_date: "2026-06-01".into(),
        initial_cash: 10_000_000.0,                   // 期初资金
        strategy: StrategySpec::topk_dropout(100, 100),
        exchange: ExchangeParams::default(),          // 费率/滑点/阈值默认值见 §4.2
        benchmark_name: rust_bt::BenchmarkName::Zz1000,
        excess_method: rust_bt::ExcessMethod::Arithmetic,
        progress: false,                              // 终端进度条（stderr），嵌入场景通常关闭
    };

    // 信号从 pred.csv 加载；一次调用完成装配 + 回测 + 报告
    let output = run_from_signal_file(params, "tmp_data/pred.csv")?;

    // 简报：关键指标一段文本（完整指标见 §6）
    println!("{}", output.report.summary());
    Ok(())
}
```

## 3. 接口总览

### 入口函数（`rust_bt::api`，均已从 crate 根 re-export）

| 函数 | 签名 | 用途 |
| --- | --- | --- |
| `run` | `fn run(params: BtParams, signal: &Signal) -> Result<BtOutput>` | 核心入口：装配 -> 校验 -> 加载数据 -> 主循环 -> 报告 |
| `run_from_signal_file` | `fn run_from_signal_file(params: BtParams, signal_path: &str) -> Result<BtOutput>` | 便捷入口：等价于先 `load_signal(path)` 再 `run` |
| `signal_from_pairs` | `fn signal_from_pairs(days: BTreeMap<NaiveDate, Vec<(String, f64)>>) -> Result<Signal>` | 内存信号便捷构造（日期 -> (instrument, score) 列表） |
| `signal_from_dataframe` | `fn signal_from_dataframe(df: &polars::prelude::DataFrame) -> Result<Signal>` | 直接接收 polars DataFrame（列要求同 pred.csv，见 §5） |

### 相关类型

| 类型 | 说明 |
| --- | --- |
| `BtParams` | 回测参数（§4） |
| `StrategySpec` | 策略规格：`TopkDropout{..}` 参数化 / `Custom(Box<dyn Strategy>)` 注入 |
| `ExchangeParams` | 撮合与成本参数（实现 `Default`，默认值对齐 CLI） |
| `BtOutput` | 回测输出：`result: BTResult` + `report: Report`（§6） |
| `ExportNames` | 导出产物文件名（实现 `Default`） |

### 便捷入口内部流程（与组件层等价）

数值参数校验（`initial_cash > 0`、`top_n >= 1` 等，`Err` 类型 `BtError::InvalidParam`）
-> 装配（`Exchange::new` 费用/阈值校验）-> 加载行情与基准 CSV -> 主循环 ->
`gen_report`（基准覆盖校验）。**校验先于数百 MB 行情加载**（参数错误 fail fast）。

## 4. 参数详解 BtParams

### 4.1 顶层字段

| 字段 | 类型 | 约束 / 说明 |
| --- | --- | --- |
| `stock_bar` | `String` | 股票日行情 CSV 路径；交易日历由其中全部 `datetime` 去重排序构成 |
| `benchmark` | `String` | 基准收益 CSV 路径；一个文件可含多个指数，报告按 `benchmark_name` 选定 |
| `start_date` / `end_date` | `String` | `YYYY-MM-DD`；闭开区间 `[start, end)`，按交易日历自动对齐 |
| `initial_cash` | `f64` | 期初资金，须为正的有限值 |
| `strategy` | `StrategySpec` | 见 §4.3 与 §8 |
| `exchange` | `ExchangeParams` | 见 §4.2 |
| `benchmark_name` | `BenchmarkName` | 枚举：`Hs300` / `Zz500` / `Cyb` / `Zz800` / `Zz1000`（默认常用）/ `Zz2000` / `Sci` / `Kci` / `Cyi`，对应规范"基准名称映射" |
| `excess_method` | `ExcessMethod` | `Arithmetic`（r − b）/ `Geometric`（(1+r)/(1+b) − 1） |
| `progress` | `bool` | 进度条渲染到 stderr；嵌入场景（服务、测试、重定向输出）应关闭 |

参数枚举（`DealPrice` / `BenchmarkName` / `ExcessMethod`）在**编译期**约束取值，
不存在 YAML 场景下的拼写错误问题。

### 4.2 ExchangeParams（默认值与 `config.example.yml` 一致）

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `deal_price` | `DealPrice` | `Open` | 成交价列：`Open` / `Close` / `Vwap` |
| `open_cost` | `f64` | `0.00015` | 买入费率（万 1.5） |
| `close_cost` | `f64` | `0.00065` | 卖出费率（佣金 + 印花税，万 6.5） |
| `min_cost` | `f64` | `5.0` | 单笔最低费用（元） |
| `fixed_slippage` | `f64` | `0.01` | 固定滑点（元） |
| `min_slippage_ratio` | `f64` | `0.0014` | 最小滑点比例 |
| `volume_threshold` | `Option<f64>` | `Some(0.5)` | 成交量限制比例（当日可成交上限 = volume × threshold）；`None` 不限制 |
| `limit_threshold` | `Option<f64>` | `Some(0.0985)` | 涨跌停判定阈值；`None` 不做涨跌停限制（告警提示） |

费率/滑点须非负、`limit_threshold` 须在 `(0, 0.1]`，违规在数据加载前返回
`BtError::InvalidParam`。

### 4.3 StrategySpec

```rust
pub enum StrategySpec {
    TopkDropout { top_n: usize, drop_n: usize, only_tradable: bool, forbid_st: bool },
    Custom(Box<dyn Strategy>),
}
```

- `StrategySpec::topk_dropout(top_n, drop_n)`：快捷构造（`only_tradable` / `forbid_st` 为 false）。
  语义：目标持仓为 score 最高的 `top_n` 只；每个有信号的交易日卖出持仓中 score
  最差的 `drop_n` 只，再等权买入新股票补足。
- `Custom(...)`：注入自定义策略（实现 `Strategy` trait，见 §8）。`run` 会**消耗**
  该 Box（装配进 `Backtest`）；跨多次回测复用同一策略实例请每次重新构造。

## 5. 信号：文件加载与内存构造

四种来源任选，`run` 统一接收 `&Signal`：

```rust
// A. CSV 文件（datetime, instrument, score [, ret]；ret 加载时剥离，防前视）
let signal = rust_bt::load_signal("pred.csv")?;

// B. polars DataFrame 直连（列要求与 pred.csv 相同；校验口径与 A 完全一致）
//    嵌入方因子管线产出的 DataFrame 无需落盘，直接进入回测
let df: polars::prelude::DataFrame = my_factor_model::daily_scores_df()?;
let signal = rust_bt::signal_from_dataframe(&df)?;

// C. 内存便捷构造：日期 -> (instrument, score) 列表
use std::collections::BTreeMap;
let mut days = BTreeMap::new();
days.insert(
    chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(),
    vec![("SH600000".to_string(), 0.95), ("SZ000001".to_string(), 0.80)],
);
let signal = rust_bt::signal_from_pairs(days)?;

// D. 分日构造（需要逐日控制校验时机时）
let day = rust_bt::SignalDay::from_pairs(vec![("SH600000".to_string(), 0.95)]).unwrap();
let signal = rust_bt::Signal::from_days(BTreeMap::from([
    (chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(), day),
]));
// 枚举信号日：signal.dates() -> Iterator<Item = NaiveDate>（升序）
// 反查某日：signal.get(&date) -> Option<&SignalDay>（as_map() 得 HashMap<Code, f64>）
```

**DataFrame 列要求**（方式 B）：必需 `datetime` / `instrument` / `score` 三列
（datetime 与 instrument cast 为 String、score cast 为 f64，cast 失败返回 polars
错误）；多余列忽略--`ret` 同样被剥离，结构上不进入回测。日期列推荐
`YYYY-MM-DD` 字符串格式。

**校验口径**（四种方式完全一致，B/A 为同一实现）：

| 规则 | 处理 |
| --- | --- |
| 同一信号日 instrument 重复 | `Err`（`BtError::Validation`） |
| instrument 无法解析（非 SH/SZ + 6 位数字且前缀匹配） | 该条丢弃 + warning |
| score 缺失 / NaN / 非有限 | 该条丢弃 + warning |
| 信号日不在交易日历 / instrument 无行情数据 | 推迟到 `run` 启动时丢弃 + warning |

信号为 **T−1 日信息**：回测在 T_exec 日消费前一交易日的信号（规范"信息边界"）。

## 6. 结果消费 BtOutput

```rust
pub struct BtOutput {
    pub result: BTResult,   // 逐日账户 / 成交 / 持仓
    pub report: Report,     // 指标与序列
}
```

### 6.1 BTResult（`output.result`）

| 方法 | 返回 | 说明 |
| --- | --- | --- |
| `daily()` | `&[DailyRecord]` | 逐日账户，每交易日一条（区间对齐后） |
| `trades()` | `&[TradeRecord]` | 逐笔成交，**含未成交订单**（`deal_volume = 0`） |
| `hist_positions()` | `&[HistPositionRow]` | 逐日持仓快照（每持仓股每日一行） |
| `elapsed()` | `Duration` | 主循环墙钟耗时（不含数据加载） |
| `export_hist_position(path)` / `export_trades(path)` | `Result<()>` | 单产物导出（组件层接口，`export_all` 内部复用） |
| `gen_report(name, method)` | `Result<Report>` | 由基准名重新生成报告（`BtOutput.report` 已含一次） |

字段明细（`DayIdx` / `Code` 均为 `u32` 内部主键，见 §11 的转换约定）：

```rust
DailyRecord    { day: DayIdx, account: f64, value: f64, cash: f64,
                 turnover_amount: f64, cost: f64 }          // account=当日总资产(含成本口径)
TradeRecord    { day, stock: Code, side: Side, volume, price,
                 deal_volume, deal_price, deal_cost }        // volume/deal_volume 存绝对值
HistPositionRow { day, code: Code, volume, cost_price, price, count_day }
```

### 6.2 Report（`output.report`）

**简报**（关键指标一段文本，适合日志 / 终端 / 通知；CLI 尾部输出同款）：

```rust
println!("{}", output.report.summary());
```

```text
回测区间: 2026-01-05 ~ 2026-05-29（95 个交易日，基准 zz1000 / arithmetic）
区间收益率:    -18.17%（含成本）/   -12.87%（不含成本）
年化收益率:    -41.25%（含成本）/   -30.61%（不含成本）
年化波动率:     24.84%
夏普比率:       -2.01
最大回撤:       23.51%
超额年化:      -55.38%（含成本）/   -47.40%（不含成本）
信息比率:      -10.05（含成本）/    -8.01（不含成本）
平均日换手率:  149.94%
```

衍生指标（`output.report.derived: DerivedStats`，全部含成本 / 不含成本双口径）：

| 字段 | 说明 |
| --- | --- |
| `annualized_return` / `annualized_volatility` / `sharpe` | 年化收益 / 年化波动（ddof=0）/ 夏普（无风险 0） |
| `max_drawdown` | 最大回撤（含成本净值） |
| `excess_annualized_return` / `information_ratio` | 超额年化 / 信息比率 |
| `annualized_return_without_cost` / `excess_annualized_return_without_cost` / `information_ratio_without_cost` | 不含成本口径（V + 累计费用近似） |

逐日序列访问器（均与 `dates()` 等长，首日为基期：净值 1、收益 0）：

```rust
output.report.dates();               // -> &[NaiveDate]
output.report.metrics();             // -> &DataFrame（export_data 同构逐 bar 表）
output.report.cum_with_cost();       // -> &[f64]  累计净值（含成本）
output.report.cum_without_cost();    // 不含成本口径累计净值
output.report.cum_benchmark();       // 基准累计净值
output.report.drawdown();            // 回撤序列（含成本，正值）
output.report.drawdown_without();    // 回撤序列（不含成本）
output.report.cum_excess();          // 累计超额净值（含成本口径）
output.report.cum_excess_without();  // 累计超额净值（不含成本）
output.report.excess_drawdown();     // 超额净值回撤（含成本）
output.report.excess_drawdown_without();
output.report.turnover();            // 双边换手率（日度）
```

## 7. 文件导出

```rust
// 默认文件名：hist_position.csv / trades.csv / report_data.csv / report_plot.html
output.export_all("output/", &ExportNames::default())?;

// 自定义文件名（目录不存在自动创建）
let names = ExportNames {
    hist_position: "positions.csv".into(),
    trades: "trades.csv".into(),
    report_data: "metrics.csv".into(),
    report_plot: "report.html".into(),
};
output.export_all("out/dir", &names)?;
```

产物内容与 CLI 完全一致（规范"数据文件格式"）：hist_position（含 weight 列）、
trades（含未成交行）、report_data（逐 bar 指标表）、report_plot（自包含交互式 HTML）。

## 8. 自定义策略

实现 `Strategy` trait 并经 `StrategySpec::Custom` 注入。策略可见信息由
`StrategyContext` **编译期限定**（防前视，架构 D4）：T−1 信号、当前持仓、现金、
当日可交易性、日期索引--**不存在**未来收益字段（`ret` 在信号加载时已剥离）。

```rust
pub trait Strategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision>;

    // 买单核减钩子（可选覆写）：阶段一卖出全部撮合后、买入撮合前调用；
    // 默认按 target_positions 截断（top_n − 卖出后实际持仓数，从尾部丢弃）
    fn revise_buy_orders(&self, buys: Vec<Order>, after_sell: &PostSellContext)
        -> Result<Vec<Order>> { /* 默认实现 */ }
}
```

`Decision` 结构与订单约定：

```rust
Decision {
    sell_orders: Vec<Order>,        // 卖单（任意顺序，全部先撮合，回款当日可用）
    buy_orders:   Vec<Order>,       // 买单按优先级降序（核减时从尾部丢弃低优先级）
    target_positions: Option<usize>, // 默认核减钩子的目标持仓数；None 不核减
}
Order::new(stock: Code, volume: f64, price: f64)  // volume 买为正、卖为负；
                                                  // price 取当日 deal_price 列
```

完整示例：**全仓轮动单只最高分股票**

```rust
use rust_bt::{Code, Decision, Order, Result, Strategy, StrategyContext};

pub struct Top1Rotation;

impl Strategy for Top1Rotation {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision> {
        // 1. 目标 = 当日 score 最高且可买入的股票
        let target = ctx
            .signal
            .codes
            .iter()
            .copied()
            .zip(ctx.signal.scores.iter().copied())
            .filter(|(c, _)| ctx.tradable.get(*c).is_some_and(|t| t.buyable()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(code, _)| code);
        let Some(target) = target else {
            return Ok(Decision::default()); // 无可买候选，今日不动作
        };

        // 2. 卖出全部非目标持仓（不可交易的自然卖不掉，留在持仓）
        let mut sell_orders = Vec::new();
        for (&code, entry) in ctx.positions.iter() {
            if code == target || !ctx.tradable.get(code).is_some_and(|t| t.sellable()) {
                continue;
            }
            let price = ctx.tradable.get(code).unwrap().deal_price;
            sell_orders.push(Order::new(code, -entry.volume, price));
        }

        // 3. 全仓买入目标（现金 + 预期回款毛额，按 100 股整手向下取整）
        let mut buy_orders = Vec::new();
        if !ctx.positions.contains_key(&target) {
            let t = ctx.tradable.get(target).unwrap();
            let proceeds: f64 = sell_orders
                .iter()
                .map(|o| -o.volume * ctx.tradable.get(o.stock).unwrap().deal_price)
                .sum();
            let budget = ctx.cash + proceeds;
            let lots = (budget / t.deal_price / 100.0).floor();
            if lots > 0.0 {
                buy_orders.push(Order::new(target, lots * 100.0, t.deal_price));
            }
        }

        Ok(Decision { sell_orders, buy_orders, target_positions: Some(1) })
    }
}

// 注入：
// let params = BtParams { strategy: StrategySpec::Custom(Box::new(Top1Rotation)), .. };
```

`ctx.tradable.get(code)` 返回 `Option<StockTradable>`（`None` = 当日无行情）：

```rust
StockTradable {
    suspended: bool,   // 停牌（paused=1 或 close 缺失）
    limit_buy: bool,   // 涨停不可买（按 deal_price 列对 pre_close 判定）
    limit_sell: bool,  // 跌停不可卖
    volume_cap: f64,   // 当日可成交上限（volume × threshold；无量为 0）
    deal_price: f64,   // 当日成交价列（无效为 NaN）
    is_st: bool,       // ST 标记（盘前公开信息）
}
// 快捷判定：t.buyable() / t.sellable()
```

撮合规则（滑点叠加、资金约束反解、整手、费用）由 Exchange 统一执行，策略无需
也无法自行模拟；未成交订单自动回填 `deal_volume = 0`。同一交易步内对同一股票
同时买 + 卖会返回 `BtError::InvalidDecision`。

## 9. 错误处理

`run` 系列返回 `rust_bt::Result<T>`（`Result<T, BtError>`），`BtError` 实现
`std::error::Error`（thiserror），可直接 `?` 进 `anyhow`：

| 变体 | 场景 |
| --- | --- |
| `InvalidParam(String)` | 数值参数越界（initial_cash ≤ 0、费率为负、limit_threshold 越界等） |
| `Validation(String)` | 数据校验失败（重复键、缺列、factor 非法；内存信号重复 instrument） |
| `Calendar(String)` | 区间对齐失败（start ≥ end、区间内无交易日）、日期格式非法 |
| `BenchmarkCoverage(String)` | 基准未覆盖回测区间全部交易日 |
| `InvalidDecision(String)` | 策略决策非法（同股同时买 + 卖） |
| `Polars(..)` / `Io(..)` | CSV 解析 / 文件 IO 错误 |

参数类错误（`InvalidParam` / `Validation` 的参数部分）发生在行情加载**之前**。

## 10. 调用示例集

### 示例 1：研究循环 -- 内存信号（从自己的因子模型生成）

```rust
use std::collections::BTreeMap;
use chrono::NaiveDate;
use rust_bt::{run, signal_from_pairs, BtParams, ExchangeParams, StrategySpec};

fn research_loop(stock: &str, bench: &str) -> rust_bt::Result<f64> {
    // 1. 信号来自你的因子模型（示例：任意生成器），按日收集 (instrument, score)
    let mut days: BTreeMap<NaiveDate, Vec<(String, f64)>> = BTreeMap::new();
    for (date, inst, score) in my_factor_model::daily_scores() {
        days.entry(date).or_default().push((inst.to_string(), score));
    }

    // 2. 构造一次，后续多次回测可复用（run 只借用 &Signal）
    let signal = signal_from_pairs(days)?;

    // 3. 单次回测取信息比率
    let params = BtParams {
        stock_bar: stock.into(),
        benchmark: bench.into(),
        start_date: "2026-01-01".into(),
        end_date: "2026-06-01".into(),
        initial_cash: 10_000_000.0,
        strategy: StrategySpec::topk_dropout(100, 100),
        exchange: ExchangeParams::default(),
        benchmark_name: rust_bt::BenchmarkName::Zz1000,
        excess_method: rust_bt::ExcessMethod::Arithmetic,
        progress: false,
    };
    let output = run(params, &signal)?;
    Ok(output.report.derived.information_ratio)
}
```

### 示例 2：参数扫描 -- top_n 网格

```rust
use rust_bt::{run, signal_from_pairs, BtParams, ExchangeParams, StrategySpec};

fn sweep(signal: &rust_bt::Signal, paths_stock: &str, paths_bench: &str) -> rust_bt::Result<()> {
    let mut results = Vec::new();
    for top_n in [20, 50, 100, 200] {
        let params = BtParams {
            stock_bar: paths_stock.into(),
            benchmark: paths_bench.into(),
            start_date: "2026-01-01".into(),
            end_date: "2026-06-01".into(),
            initial_cash: 10_000_000.0,
            strategy: StrategySpec::topk_dropout(top_n, top_n),
            exchange: ExchangeParams::default(),
            benchmark_name: rust_bt::BenchmarkName::Zz1000,
            excess_method: rust_bt::ExcessMethod::Arithmetic,
            progress: false,
        };
        let output = run(params, signal)?;
        results.push((top_n, output.report.derived.sharpe));
        println!("top_n={top_n:>3}  sharpe={:.4}", results.last().unwrap().1);
    }
    Ok(())
}
```

> 每次调用 `run` 重新加载行情 CSV（当前无跨 `run` 的数据复用；架构 D13 记录了
> 后续 `Arc` 共享行情的演进方向）。数据量大、组合数多时请把加载耗时计入预算。

### 示例 3：基于序列计算自定义指标（卡玛比率 + 平均换手）

```rust
let output = rust_bt::run(params, &signal)?;

// 卡玛比率 = 年化收益 / 最大回撤
let calmar = output.report.derived.annualized_return / output.report.derived.max_drawdown;

// 平均日换手与超额创新高占比（直接读序列，无需落盘再解析 CSV）
let avg_turnover: f64 = output.report.turnover().iter().sum::<f64>()
    / output.report.turnover().len() as f64;
let peak = output.report.cum_excess().iter().fold(f64::MIN, f64::max);
let at_high = output.report.cum_excess().iter()
    .filter(|v| (**v - peak).abs() < 1e-12).count();

println!("calmar={calmar:.4} avg_turnover={avg_turnover:.4} 超额新高天数={at_high}");
```

### 示例 4：只跑回测、不生成报告（组件层）

`run` 总是生成报告（需要 benchmark 数据覆盖区间）。只需成交与账户序列时用组件层：

```rust
use rust_bt::*;

let signal = load_signal("pred.csv")?;
let data = BTData::new()
    .load_stock_bar("stock_bar.csv")?
    .build()?;                          // benchmark 可不加载
let account = Account::new(10_000_000.0);
let exchange = Exchange::new("open", 0.00015, 0.00065, 5.0, 0.01, 0.0014,
                             Some(0.5), Some(0.0985))?;
let strategy: Box<dyn Strategy> = Box::new(TopkDropoutStrategy::new(100, 100));
let mut bt = Backtest::new(data, account, exchange, strategy);
let result = bt.run(&signal, "2026-01-01", "2026-06-01")?;
for d in result.daily() {
    // ... 直接消费逐日账户
}
```

组件层注意：`Backtest::run` **只能调用一次**（内部状态被消耗），再次回测需重新
装配；进度条用 `with_progress(true)` 打开。

## 11. 注意事项

- **instrument 与 Code**：字符串 instrument（`SH600000`）只存在于 IO 边界；
  结果中的 `stock` / `code` 字段为 `u32`（`600000`）。转换用
  `rust_bt::parse_instrument("SH600000") -> Result<Code>` 与
  `rust_bt::format_instrument(600000) -> Result<String>`。
- **DayIdx**：交易日索引（区间内 0 起），非自然日；导出文件中已格式化为
  `YYYY-MM-DD`。
- **信息边界**：策略与信号均无未来收益信息（`ret` 列加载时剥离，结构上不存在）；
  给 `SignalDay`/策略上下文新增字段前须评估是否构成前视（架构 D4）。
- **费用口径**：费用直接扣现金、不摊入 `cost_price`；不含成本指标由
  `V + 累计费用` 在 Report 层派生（近似，架构 D5）。现金允许为负（卖出费用超
  成交金额情形）。
- **单次性**：`api::run` 每次内部新建 `Backtest`，无单次限制；组件层
  `Backtest::run` 只能调用一次。
- **日志**：告警走 `log` facade，调用方负责初始化 logger（§1）。

---

*本文由代码生成于 2026-08；与代码不一致时以 [`doc/specification.md`](specification.md)
为准，并请提 issue 修正本文。*
