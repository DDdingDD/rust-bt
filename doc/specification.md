# Rust 股票回测系统 —— 设计规范

> 

## 简介

- 开发语言：Rust
- 目前只支持 A 股市场：暂时仅支持上交所与深交所，不支持北交所（后续会支持）
- 目前只支持日频回测
- 使用 polars 处理数据，减少 for 循环

## 使用方法

```rust
fn main() -> anyhow::Result<()> {
    // 1. 加载信号
    let signal = load_signal("pred.csv")?; // CHANGE PATH IF NEEDED

    // 2. 策略参数
    let top_n = 100;
    let drop_n = 100;

    // 3. 资金与交易成本参数
    let cash = 10_000_000.0; // 1000 万
    let deal_price = "open";
    let open_cost = 0.00015; // 万 1.5
    let close_cost = 0.00065; // 万 6.5（佣金 + 卖出印花税）
    let min_cost = 5.0;
    let fixed_slippage = 0.01;
    let min_slippage_ratio = 0.0014;
    let volume_threshold = Some(0.5);
    let limit_threshold = Some(0.0985);

    // 4. 回测区间：闭开区间 [start_date, end_date），自动按交易日历对齐
    let start_date = "2026-01-01";
    let end_date = "2026-06-01";

    // 5. 加载行情与基准数据
    let data = BTData::new()
        .load_stock_bar("stock_bar.csv")?
        .load_benchmark("benchmark.csv")?
        .build()?;

    // 6. 账户 / 交易所 / 策略
    let account = Account::new(cash);
    let exchange = Exchange::new(
        deal_price,
        open_cost,
        close_cost,
        min_cost,
        fixed_slippage,
        min_slippage_ratio,
        volume_threshold,
        limit_threshold,
    )?;
    let strategy: Box<dyn Strategy> = Box::new(TopkDropoutStrategy::new(top_n, drop_n));

    // 7. 运行回测（with_progress 启用终端进度条，默认关闭）
    let mut backtest = Backtest::new(data, account, exchange, strategy)
        .with_progress(true);
    let bt_result = backtest.run(&signal, start_date, end_date)?;

    // 8. 输出结果
    bt_result.export_hist_position("hist_position.csv")?;
    bt_result.export_trades("trades.csv")?;
    let report = bt_result.gen_report("zz1000", "arithmetic")?;
    report.export_data("report_data.csv")?;
    report.plot()?;

    Ok(())
}
```

---

## 接口概要

示例中各入口的签名约定（示意，非完整定义）：

```rust
// 信号：加载 pred.csv，完成结构校验（(datetime, instrument) 重复、score 缺失/NaN）并剥离 ret 列；
// 日历/行情相关校验（datetime 不在交易日历、instrument 无行情）依赖交易日历，推迟到 Backtest::run 启动时执行
fn load_signal(path: &str) -> anyhow::Result<Signal>;

// 数据容器：加载行情与基准，build 时统一校验并构建交易日历
impl BTData {
    fn new() -> Self;
    fn load_stock_bar(self, path: &str) -> anyhow::Result<Self>;
    fn load_benchmark(self, path: &str) -> anyhow::Result<Self>;
    fn build(self) -> anyhow::Result<BTData>;
}

// 回测执行器：终端进度条开关（默认关闭；行为见"核心概念--Backtest"）
impl Backtest {
    fn with_progress(self, enabled: bool) -> Self;
}

// 回测结果：逐日账户快照 + 持仓历史 + 成交记录
impl BTResult {
    fn export_hist_position(&self, path: &str) -> anyhow::Result<()>;
    fn export_trades(&self, path: &str) -> anyhow::Result<()>;
    fn gen_report(&self, benchmark: &str, excess_method: &str) -> anyhow::Result<Report>;
    fn gen_report_default(&self) -> anyhow::Result<Report>; // 等价于 gen_report("zz1000", "arithmetic")
    fn elapsed(&self) -> std::time::Duration; // run() 墙钟耗时（不含 BTData 加载），进度条关闭时同样记录
}

// 报告：export_data 输出逐 bar 原始指标；plot 绘制净值/回撤/超额曲线并输出 PNG
impl Report {
    fn export_data(&self, path: &str) -> anyhow::Result<()>;
    fn plot(&self) -> anyhow::Result<()>; // 输出 report_plot.png：净值 / 回撤 / 超额三条曲线，X 轴为交易日
}
```

> 字符串枚举参数（`deal_price`、benchmark 名称、`excess_method`）取值非法时一律返回 `Err`，不 panic；实现上建议用枚举类型承载。

---

## 约定

### 数值与单位

- 金额：人民币元，`f64`。
- 价格：元/股，`f64`，为**不复权原始价**（复权由 `factor` 处理，见"复权处理"）。
- 数量：股，`f64` 存储、撮合后取整为整手（见"整手取整"）。
- 日期：`YYYY-MM-DD` 字符串 / `Date` 类型。

### 交易日历

- 交易日历 = `stock_bar.csv` 中去重排序后的全部 `datetime`。
- 回测区间 `[start_date, end_date)` 对齐方式：实际首日 = 区间内第一个交易日，实际末日 = 区间内最后一个交易日（不含 end_date）。
- 回测区间校验：`start_date ≥ end_date`，或区间内不含任何交易日，直接报错。
- 基准覆盖校验：基准数据必须覆盖回测区间的全部交易日，否则**直接报错**。校验在 `gen_report(name)` 选定基准后进行（加载阶段不校验，因为一个文件含多个基准指数）。基准中不在交易日历内的行（如非交易日记录）忽略。

### 数据校验

校验分两阶段：`load_*` 阶段做结构校验；pred 的日历/行情相关校验依赖交易日历，推迟到 `Backtest::run` 启动时执行。违规即报错（除非另行说明）：

| 数据        | 规则                                                | 处理                                                    |
| --------- | ------------------------------------------------- | ----------------------------------------------------- |
| stock_bar | (datetime, instrument) 重复                         | 报错                                                    |
| stock_bar | `paused = 1` 或 `close` 缺失                         | 该日该股不可交易（非错误）                                         |
| stock_bar | 当日 `volume` 缺失或为 0                                | 该日该股不可交易（非错误；无量即无对手盘，**与 `volume_threshold` 是否设置无关**） |
| stock_bar | `deal_price` 对应价格列缺失 / NaN / ≤ 0             | 该日该股不可交易（非错误；如无成交日 `vwap = money / volume` 无效）  |
| stock_bar | `pre_close` 缺失（如上市首日）                             | 该股当日跳过涨跌停预计算，`limit_buy`/`limit_sell` 置 false         |
| stock_bar | `high_limit` / `low_limit` 缺失（非 `pre_close` 缺失引起） | 同上：当日不做涨跌停判定，置 false 并 warning                        |
| stock_bar | 必需列缺失（约定列不存在）                                  | 报错                                                    |
| stock_bar | 价格列（open/close/high/low）≤ 0，或 `high < low`，或 `volume < 0` | 报错                                          |
| stock_bar | `factor` 缺失 / NaN / ≤ 0                          | 报错（复权依赖该列，异常值会静默破坏持仓调整）                    |
| stock_bar | `paused` / `is_st` 值缺失                            | 按 0 处理并 warning                                       |
| benchmark | (datetime, instrument) 重复                         | 报错                                                    |
| pred      | (datetime, instrument) 重复                         | 报错                                                    |
| pred      | `datetime` 不在交易日历中                                | 该条信号丢弃并 warning                                       |
| pred      | `score` 为 NaN / 缺失                                | 该条信号丢弃并 warning                                       |
| pred      | 信号中的 instrument 无行情数据                             | 该条信号丢弃并 warning                                       |

---

## 核心概念

### Signal（信号）

策略做决策所依赖的预测分数，来自 `pred.csv`。

> **可见性约束**：信号中的 `ret` 列（未来收益率）仅供离线评估信号质量（如 IC 分析），**回测引擎与策略不可见**，实现上应在加载后剥离或隔离，防止前视偏差。

### Strategy（策略）

策略接口：输入 T−1 日信号与 T_exec 日账户状态，输出 Decision：

```text
gen_decision(signal, position, cash, tradable_info) -> Decision
```

**信息边界（防前视）**，策略在 T_exec 日可见的信息：

- T−1 日信号（`score`，已剥离 `ret`）；
- 复权调整后的当前持仓与可用现金；
- `tradable_info`：T_exec 日各股可交易性--`paused` / `limit_buy` / `limit_sell` / 当日成交量上限（`volume × volume_threshold`；`volume_threshold = None` 时为 ∞，不设限）；
- T_exec 日 `deal_price` 列（用于委托定价与金额换算）。

涨跌停基于当日 `deal_price` 对 `pre_close` 判定：`deal_price = "open"` 时开盘竞价结束即可得，属合法可见；`deal_price = "close" / "vwap"` 时判定价格与成交价在决策时点不可得，属于"以收盘价/全日均价成交"的**简化假设**（研究近似：信号仍为 T−1 日，不构成对盘后数据的策略性利用）。策略**不可见**当日行情的其他列，也不可见信号 `ret` 列。内置策略委托价格统一取当日 `deal_price`。

同样属于简化假设：`tradable_info` 的当日成交量上限取自**全日**成交量，在 `deal_price = "open"` 时亦为盘后信息（开盘时不可得）。容量约束按全日量近似，与 `close` / `vwap` 成交价的前视一并接受，不视为前视偏差漏洞。

### Decision（决策）

策略在一个交易步内做出的交易决定，本质上是一个 Order 列表。

### Order（订单）

| 字段            | 含义                      |
| ------------- | ----------------------- |
| `stock`       | 股票代码                    |
| `volume`      | 委托数量（股，买为正、卖为负）         |
| `price`       | 委托价格                    |
| `deal_volume` | 实际成交数量，由 Exchange 成交后回填 |
| `deal_price`  | 实际成交价格，由 Exchange 成交后回填 |
| `deal_cost`   | 实际交易费用，由 Exchange 成交后回填 |

> `price` 由策略在生成订单时填写，内置策略取 T_exec 日行情的 `deal_price` 列（见 Strategy 信息边界）；Exchange 在该价格上叠加滑点得到订单的 `deal_price` 回填。注意与行情列名 `deal_price` 区分：前者是成交回填字段，后者是价格列选择参数。

### Backtest（回测执行器）

回测流程的编排者（即执行器 Executor 的具体实现）。按交易日历逐步推进：

1. 调用策略生成 Decision；
2. 把 Decision 中的 Order 交给 Exchange 撮合；
3. 每个 bar 结束时触发账户记账与指标记录。

进度与耗时：

- `with_progress(true)` 启用终端进度条，默认关闭（库默认零终端输出，测试与输出重定向环境干净）；渲染到 **stderr**，不污染 stdout；按交易日推进，总日数 = 对齐区间交易日数（`run` 启动校验后即知，进度与 ETA 确定），结束行显示总耗时。
- 无论开关与否，`run()` 的墙钟耗时（含启动校验与结果装配，**不含 BTData 加载**）都记入 `BTResult::elapsed()`；数据加载阶段的耗时如需统计，由调用侧在 `BTData` 构建前后自行计时。
- 进度条只读不写回测状态：开关与否不改变逐日账户序列与成交记录（验收不变量之一）。

### Exchange（交易所）

模拟交易所。行情数据由 `Backtest::new` 装配时注入（`Exchange::new` 只接收费用与约束参数），注入时按 `deal_price` 预计算 `limit_buy` / `limit_sell`（见 limit_threshold）。判断股票可交易性（停牌、涨跌停），撮合订单（成交量裁剪、滑点、手续费、整手取整、资金/持仓约束），成交后回调账户落账。

- 可交易性判断：`check_stock_suspended`（停牌）、`check_stock_limit`（涨跌停）、`is_stock_tradable`（是否可交易）
- 撮合入口：`deal_order`

### Account（账户）

回测交易账户，负责记账、收益计算、组合指标与交易指标的更新。是 Exchange 撮合结果的落账方，也是 Report 指标的记录者。

账户层面持有：`cash`（可用现金）、`account_value`（总资产快照）、`current_position`（持仓）。此外同步维护不含费用口径的总资产快照（见"指标定义--不含成本口径"）。

### Position（持仓）

持仓数据的容器与计算器，由 Account 的 `current_position` 指向。包含一个持股列表，每项为：

| 字段            | 含义                                   |
| ------------- | ------------------------------------ |
| `stock`       | 股票代码                                 |
| `volume`      | 持有数量                                 |
| `cost_price`  | 持仓成本价（用于盈亏计算）                        |
| `price`       | 最新收盘价（用于市值估值）                        |
| `weight`      | 权重（= 该持仓市值 / 当日总资产，含成本口径，日终估值后计算）    |
| `last_factor` | 最近一次入账时的复权因子（见"复权处理"）                |
| `count_day`   | 连续持仓天数（语义同 hist_position 输出，见数据文件格式） |

> 注意：`cost_price` 与 `price` 必须同时存在——前者记账算盈亏，后者每日估值，二者不可合并。

### Report（报告）

回测结果的指标容器。目前实现：

- `PortfolioMetrics`：组合层面逐 bar 指标——总资产、收益率、换手率、成本率、持仓市值、现金、基准收益（由 `gen_report` 显式指定；便捷入口 `gen_report_default()` 取 **zz1000**）。

---

## 回测主循环时序

回测区间内的每个交易日 T_exec 按以下顺序执行：

```text
for T_exec in 交易日历 ∩ [start_date, end_date):
    1. 取信号：使用前一交易日 T_exec−1 的 score（T 日收盘生成，T+1 执行）；
       前一交易日无信号（含 start_date 之前无数据）时，本日不产生任何订单，持仓不动。
    2. 复权调整：对持仓中当日 factor 与其 last_factor 不同的股票，调整 cost_price 与 volume（见"复权处理"）。
       调整先于撮合执行：除权日送转增加的股数当日即可卖出；当日新买入的持仓此时尚未入账，
       天然不参与本次调整（其成交价已是除权后价格）。
    3. 生成决策：strategy.gen_decision(signal, current_position, cash, tradable_info) → Order 列表。
    4. 撮合：Exchange.deal_order 逐单撮合，成交价取 T_exec 日的 deal_price 列；
       卖单先于买单处理，卖出回款（扣除费用后）当日即可用于买入。
    5. 日终估值与记账：
       - 每只持仓的 price 更新为 T_exec 日收盘价；当日停牌 / 无行情 / close 缺失的持仓沿用最近一个有效收盘价（有效 = 最近一个未停牌且 close 为正的行情行的收盘价；停牌行即使带 close 也不采用，二者数值通常相同）；
       - account_value = cash + Σ(volume × price)；
       - 记录 PortfolioMetrics 逐 bar 指标。
```

补充规则：

- **T+1**：日频回测天然满足"当日买入不可当日卖出"（买在 T_exec，最早 T_exec+1 才有卖出机会），不单独建模；卖出回款当日可用，显式支持。
- **期初空仓**：start_date 首个交易日使用的是 start_date 前一交易日的信号；若 `pred.csv` 不含该日信号，则首日空仓，首个有信号可用的交易日按策略规则建仓（TopkDropout 当日直接买入 `top_n` 只，受候选与资金约束，见内置策略第 2 条）。
- **退市处理**：持仓股终止上市（之后不再有行情记录）时，不再产生信号、不可卖出，估值沿用最后一个有效收盘价直至期末，hist_position 照常输出；不做强制平仓建模。该行为完全由"当日无行情记录"驱动，无需额外状态。

---

## 复权处理（factor）

`stock_bar.csv` 中价格为不复权原始价，`factor` 为累计复权因子。持仓跨除权除息日（`factor` 发生变化）时，如下调整持仓，保持持仓市值与真实盈亏连续：

```text
factor_ratio = factor_today / factor_last   // factor_last 见下
volume     ×= factor_ratio                  // 送转股导致股数增加
cost_price /= factor_ratio                  // 除权导致成本价降低
```

- **`factor_last` 取该持仓自身记录的 `last_factor`**（最近一次入账时的 factor），而非固定取上一交易日的行情：买入成交日记当日 factor，之后每次调整更新为当日 factor。由此：
  - 停牌 / 当日无行情记录不影响正确性，恢复后一次性补调；
  - **当日新买入的持仓不做当日调整**（其成交价已是除权后原始价，入账 factor 即当日 factor），避免双重调整；
  - 判定条件为 `factor_today ≠ last_factor`（浮点比较带 epsilon）。
- 调整在每个交易日**撮合之前**执行（主循环第 2 步），除权日卖单按送转后的最新 volume 撮合，与真实账户一致。
- 现金分红不单独入账，分红收益通过 `factor` 调整隐含体现（factor 已含分红再投资假设）。
- 撮合成交价始终使用**当日原始价**，不做复权；估值 `price` 也使用当日原始收盘价。
- 新买入股票的 `cost_price` = 实际成交价（含滑点，不含费用）。
- 送转等除权可能导致持仓 `volume` 为非整数（f64 存储，如 10 送 3.5）；卖出不受整手限制，无需舍入处理。

> 费用不摊入 `cost_price`。费用直接扣减现金。

---

## 整手取整

- **买入**：撮合数量向下取整到 100 股的整数倍（一手 100 股）；取整后不足一手则成交量为 0。
- **卖出**：允许卖出零股（取整前的全部持仓量），不做整手限制。
- **科创板（SH688xxx / SH689xxx）**：按"200 股起、之后按 1 股递增"的申报规则处理——买入数量向下取整到 1 股（不再按 100 股取整），不足 200 股则成交量为 0；卖出无整手限制，余额不足 200 股时须一次性全部卖出（本系统卖出始终为全部持仓，天然满足）。SH689 为科创板 CDR，适用同样规则。
- 取整在成交量裁剪、资金约束反解之后执行。

---

## 信号时间对齐规则（重要）

回测正确性依赖严格的时间对齐，规则如下：

- `pred.csv` 中 `datetime = T` 的 `score`：由 **T 日收盘及之前**的数据生成，T 日收盘后可用；
- 策略在 **T+1 个交易日**执行该信号产生的交易，成交价取 T+1 日的 `deal_price`；
- `ret` 列为 T+1 日收益率，仅用于信号评估，不参与回测（见 Signal 可见性约束）；
- 回测区间 `[start_date, end_date)` 为闭开区间，end_date 日不回测，非交易日自动对齐到区间内最近交易日。

---

## 内置策略

### TopkDropoutStrategy

目标持仓：预测分数最高的 `top_n` 只股票。每个**有信号可用的交易日**调仓一次，规则如下：

1. **卖出**：在当前持仓中，按当日可用信号的 `score` 升序取最差的 `drop_n` 只，全部卖出（`method_sell = "bottom"`）。持仓中当日无信号的股票不参与排名、不卖出。

2. **买入**：在未持有的股票中，按 `score` 降序选取新股票（`method_buy = "top"`）。计划买入只数统一为：
   
   ```text
   n_buy = top_n − 卖出后保留的持仓数
   ```
   
   期初空仓时持仓数为 0，首个调仓日买入 `top_n` 只；持仓满 `top_n` 且计划卖出全部成交时，退化为"卖出几只买几只"。`n_buy` 同时不超过当日可用候选（有信号且可交易）数量。

3. **资金分配**：买单等权分配——每单金额 =（可用现金 + 计划卖出全部成交的预期回款）/ 计划买入只数 `n_buy`；**预期回款为毛额口径：Σ(卖出委托量 × T_exec 日 `deal_price` 列价格)，不预估滑点与费用**（执行层摩擦对策略不可见，见信息边界），实际回款不足时由撮合阶段的资金约束反解裁剪后续买单。委托价格取 T_exec 日 `deal_price`，委托股数 = 每单金额 / 委托价格（最终以 Exchange 撮合与整手取整为准）。金额在生成决策时一次性确定：第 4 条核减买单只丢单、**不重新分配**，剩余买单保持原金额，多余现金留存。

4. **卖不掉的处理**：因跌停/停牌/成交量裁剪导致卖出失败或部分成交的股票保留在持仓中，**继续占用 `top_n` 名额**。由 Backtest 编排两阶段撮合：先处理全部卖单，随后按实际卖出结果核减买单（`n_buy` 收缩为 `top_n − 卖出成交后的实际持仓数`，按 `score` 升序丢弃多余买单），再逐笔撮合买单，避免超配与资金不足。

5. **边界情况**：
   
   - 候选股票（当日有信号、可交易）不足 `top_n` 时，持有全部候选股；
   - `score` 并列时按 `instrument` 字典序保证确定性：买入排名（降序）同分取代码小者优先，卖出排名（升序）同分取代码小者优先；
   - 某日无信号可用：不调仓，持仓原样持有；
   - 买单金额经整手取整后剩余的零散现金留作现金；
   - 持仓数已不少于 `top_n`（如历史超配）：本日不生成买单，仅按第 1 条卖出。

策略参数：

| 参数              | 类型   | 默认      | 含义                                                                                                                                                                                        |
| --------------- | ---- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `top_n`         | int  | —       | 目标持仓只数                                                                                                                                                                                    |
| `drop_n`        | int  | —       | 每个调仓日计划卖出的只数                                                                                                                                                                              |
| `only_tradable` | bool | `false` | 卖出候选是否限定当日可交易股票；两种模式的买入只数公式相同（`top_n − 卖出成交后持仓数`，见第 2/4 条）。`false`：排名与卖单包含不可交易股票，其卖单成交为 0、保留持仓并占坑（第 4 条核减买单）；`true`：不可交易股票（如跌停）不进入卖单、不参与排名，留到下一调仓日再卖。实际区别在于"末位 `drop_n` 名"里跌停股是否挤掉可交易股的名额 |
| `forbid_st`     | bool | `false` | 是否过滤 ST 股：`true` 时调仓日 `is_st = 1` 的股票不可买入；只限制买入，已持有的 ST 股照常卖出                                                                                                                             |

---

## Exchange 参数配置

### deal_price —— 成交价格

订单成交价取自行情的哪一列，当前支持 `"open"`、`"close"`、`"vwap"`（对应 `stock_bar.csv` 的 `vwap` 列）；传入不支持的值在 Exchange 构建时返回 `Err`。当日该列缺失 / NaN / ≤ 0（如无成交日 `vwap = money / volume` 无效）时该股不可交易，见"数据校验"。

### open_cost —— 买入费率

- 类型：`float`，示例值 `0.00015`（万 1.5）。
- 买入订单费用 = `max(trade_val × open_cost, min_cost)`。
- 参与现金约束：可用现金不足以支付"成交金额 + 费用"时，Exchange 反解可买股数，费用会压缩实际买入数量。

### close_cost —— 卖出费率

- 类型：`float`，示例值 `0.00065`（万 6.5，含卖出单边印花税）。
- 卖出订单费用 = `max(trade_val × close_cost, min_cost)`。
- 卖出所得现金 = `trade_val − trade_cost`。

### min_cost —— 最低费用

- 类型：`float`，默认 `5.0`（元）。
- 单笔最低手续费：`trade_cost = max(trade_val × cost_ratio, min_cost)`；成交量为 0 时费用归零。
- 现金约束反解可买数量时，按两个 regime 分别求解并取可行解中的较大者（**反解与费用计算中的 `deal_price` 均指滑点调整后的实际成交价**，即回填到 `Order.deal_price` 的口径，非行情列 / 委托价，否则会超买）：
  - 比例费 regime：`shares ≤ cash / (deal_price × (1 + cost_ratio))`，要求解落在 `shares × deal_price × cost_ratio ≥ min_cost` 区间；
  - 固定费 regime：`shares ≤ (cash − min_cost) / deal_price`，要求解落在 `shares × deal_price × cost_ratio < min_cost` 区间；
  - 现金连最低费用都不够时，订单成交量置 0。
- 卖出费用允许超过卖出成交金额（此时该笔净回款为负），不 cap 到成交金额；净回款为负时直接扣减现金，**允许现金为负**（量级为单笔最低费用），总资产与报表照常按负现金计算。

### fixed_slippage —— 固定滑点

- 类型：`float`，默认 `0.01`（元/股）。
- 以**固定金额**表示的滑点，与 `min_slippage_ratio` 共同决定实际滑点比例（见下条）。

### min_slippage_ratio —— 最小滑点比例

- 类型：`float`，默认 `0.0014`（万 14）。

- 实际滑点比例：
  
  ```text
  adj_price_ratio = max(min_slippage_ratio, fixed_slippage / trade_price)
  ```
  
  然后按方向调整成交价：
  
  - 买入：`trade_price × (1 + adj_price_ratio)`（买得更贵）；
  - 卖出：`trade_price × (1 − adj_price_ratio)`（卖得更便宜）。

- 取两者较大者的效果：
  
  - **低价股**：`fixed_slippage / price` 较大（0.01 元对 2 元股是 0.5%），固定滑点占主导；
  - **高价股**：`fixed_slippage / price` 趋近于 0，由 `min_slippage_ratio` 兜底，避免滑点比例过低失真。

- 滑点调整后的成交价可能越过当日涨停价（买入）/ 跌停价（卖出），**不做 clamp**：涨跌停约束由 `limit_buy` / `limit_sell` 在撮合前拦截，滑点是对成交价摩擦的近似，按调整后价格入账。

### volume_threshold —— 成交量限制

- 类型：`Option<float>`，默认 `None`（不限制）。
- 含义：单笔订单最大可成交量 = **当日成交量（股，`volume` 列）× 该比例**，防止回测成交超过市场容量。买入、卖出同时生效，超额部分原地裁剪。
- 当日 `volume = 0` 或缺失时，可成交量为 0，订单全部裁掉。

### limit_threshold —— 涨跌停限制

- 类型：`Option<float>`，取值范围 `(0, 0.1]`，推荐 `0.0985`；`None` 表示不做涨跌停限制（会打 warning：涨跌停的股票也能被买卖）。

- 判定逻辑：加载行情时用涨跌停价反推当日涨跌幅阈值，预计算布尔列：
  
  ```text
  up_chg   = high_limit  / pre_close − 1    // 涨停对应的涨幅
  down_chg = low_limit   / pre_close − 1    // 跌停对应的跌幅
  change   = deal_price列 / pre_close − 1   // 当日涨跌幅，取 deal_price 对应的价格列（open/close/vwap）
  
  limit_buy  = change ≥ up_chg   × (limit_threshold / 0.1)
  limit_sell = change ≤ down_chg × (limit_threshold / 0.1)
  ```
  
  其中 `change` 使用 `deal_price` 参数对应的价格列计算（如 `deal_price = "open"` 时基于开盘价判定触板）；因此 `limit_buy`/`limit_sell` 不能脱离 `deal_price` 预计算，在行情注入 Exchange 时按 deal_price 计算。`limit_threshold / 0.1` 是容差比例：如 `0.0985` 表示达到当日实际涨停幅度的 98.5% 即判定触板（对 10% 板幅股票触发线为 9.85%，距涨停约 0.15 个百分点；对 20% 板幅为 19.7%），防止买入"接近涨停、实际无法成交"的股票。板幅由 `high_limit`/`low_limit` 对 `pre_close` 反推，天然兼容 5%（ST）/10%/20% 不同板幅及 tick 舍入，公式中的 `0.1` 仅为归一化基准。

- `limit_buy = true` 时不可买入，`limit_sell = true` 时不可卖出；停牌（`paused = 1` 或 `close` 缺失）一律不可交易。**判定顺序：先停牌，后涨跌停**。停牌日的行情行常为同值 OHLC（`high_limit = low_limit = close`），其 limit 预计算会得到 `change = 0 ≥ up_chg = 0` 即 `limit_buy = true` 的无意义结果，必须由停牌检查先行拦截。

- `pre_close` 缺失（如上市首日）时该股当日不做涨跌停判定，`limit_buy`/`limit_sell` 均为 false；`high_limit` / `low_limit` 自身缺失时同样置 false 并 warning（见"数据校验"）。

> 后续可能提供其他成交价类型（如某一时段的 VWAP），届时 `change` 同样取对应价格计算；时段 VWAP 依赖分钟级数据，属于远期规划，当前日频数据仅支持 open / close / vwap（全日）。

### 撮合通用规则

- **处理顺序**：先卖后买；买单按 `score` 降序依次撮合（分数高的优先获得资金）。
- **部分成交**：允许。因成交量限制或资金不足被裁剪的订单按裁剪后数量成交，不回补、不重试。
- **资金不足反解**：买单可成交量 = min(委托量, 成交量上限, 现金反解可买股数)，再整手取整（反解用滑点调整后成交价，见 min_cost 一节）。
- **卖出超额**：卖出委托量超过持仓量时，截断至当前持仓量并打 warning。
- **当日新买入的复核**：同一订单列表中买、卖同一股票属于策略错误，直接报错；同股多笔买单合并为一笔（数量相加）。

---

## 数据文件格式

### 代码规范（instrument）

- 股票：固定 8 位，交易所前缀 + 6 位数字，`SH` 表示上海证券交易所，`SZ` 表示深圳证券交易所，如 `SH600000`、`SZ000006`；暂时仅支持上交所与深交所，**不含北交所**（BJ 前缀代码不在支持范围），后续会支持；
- 指数/基准：代码长度不固定，沿用数据源原始代码（如 `SH000300`、`CSI932000`、`CSIKC`），不参与下述 int 编码转换。
- **内部 int 编码（效率优化）**：为追求效率，加载后可将股票 `instrument` 转为 int：去掉交易所前缀，取 6 位数字部分的整数值（如 `SH000006` -> 6、`SH600000` -> 600000）。沪市股票数字段为 6xxxxx / 68xxxx，深市为 0xxxxx / 3xxxxx，两市不重叠，int 编码在股票范围内唯一（北交所数字段以 4 / 8 / 9 开头，亦不重叠，后续接入时该编码规则无需变更）；无法按该规则解析的代码按数据校验报错。命名约定：**字符串口径（`SH600000`）沿用 `instrument`，用于 CSV 输入输出与对外接口；int 口径（600000）统一用 `code` 一词**命名相关变量与列名（如 `stock_code`、polars 列 `code`），加载后正向转换、导出前反向映射回字符串。

基准名称映射（`gen_report` 的参数 → 数据文件中的 instrument）：

```json
{
    "hs300": "SH000300", "zz500": "SZ399905", "cyb":  "SZ399006",
    "zz800": "SH000906", "zz1000": "SH000852", "zz2000": "CSI932000",
    "sci":   "CSIMJSC",  "kci":   "CSIKC",    "cyi":  "CSICY"
}
```

> TODO：`benchmark.csv` 中还包含 `CSI000400`，其指数名称与别名待确认后补充进映射表。注意其数据仅覆盖至 **2025-07-04**（其余基准覆盖至 2026-08-20）：若加入映射表且回测区间超出该日，将触发基准覆盖校验报错，加入前需先补全数据。
> 
> （下述基准覆盖区间为数据快照事实，数据更换后需同步更新；`tmp_data` 仅作格式参考，不作为数据正确性依据。）各基准可用区间不一致：`CSI932000`（zz2000）自 **2020-06-20** 起（且含非交易日回填行），`CSI000400` 止于 2025-07-04，其余自 2020-01-02 起覆盖至 2026-08-20；回测区间超出所选基准覆盖范围时触发覆盖校验报错。

### stock_bar.csv —— 股票日行情

| 字段           | 含义                                                   |
| ------------ | ---------------------------------------------------- |
| `datetime`   | 交易日期                                                 |
| `instrument` | 股票代码                                                 |
| `open`       | 开盘价（不复权）                                             |
| `close`      | 收盘价（不复权）                                             |
| `low`        | 最低价                                                  |
| `high`       | 最高价                                                  |
| `volume`     | 当日成交量（股）                                             |
| `money`      | 当日成交金额（元）                                            |
| `factor`     | 累计复权因子（见"复权处理"）                                      |
| `high_limit` | 当日涨停价                                                |
| `low_limit`  | 当日跌停价                                                |
| `avg`        | 当日均价（当前与 `vwap` 数值相同，保留备用）                           |
| `pre_close`  | 前一交易日收盘价                                             |
| `paused`     | 是否停牌，1 为停牌                                           |
| `is_st`      | 是否 ST，1 为 ST                                         |
| `vwap`       | 成交均价（= `money / volume`），`deal_price = "vwap"` 时使用本列 |

> `avg` 与 `vwap` 数值相同、冗余，后续数据管道可只保留 `vwap`。当前两者都加载，`deal_price = "vwap"` 使用 `vwap` 列。

### benchmark.csv —— 基准收益

| 字段           | 含义     |
| ------------ | ------ |
| `datetime`   | 交易日期   |
| `instrument` | 基准指数代码 |
| `benchmark`  | 当日收益率  |

一个文件可包含多个基准指数；`BTData::load_benchmark` 全部加载，`gen_report(name)` 按映射表选定其中一个计算基准收益与超额收益（名称不在映射表中返回 `Err`）；便捷入口 `gen_report_default()` 等价于 `gen_report("zz1000", "arithmetic")`。

### pred.csv —— 预测信号

| 字段           | 含义                        |
| ------------ | ------------------------- |
| `datetime`   | 信号日期 T（T 日收盘后可用）          |
| `instrument` | 股票代码                      |
| `score`      | 预测分数（对 T+1 日的预测）          |
| `ret`        | T+1 日实际收益率（仅用于信号评估，回测不可见） |

### hist_position.csv —— 历史持仓输出

`export_hist_position` 的输出格式，逐交易日一行一只持仓（字段参照仓库内 `hist_position.csv`，**该文件仅作格式参考**；其日期写法 `2022/10/11` 为导出样本，实际输出统一为 `YYYY-MM-DD`）：

| 字段           | 含义                                                 |
| ------------ | -------------------------------------------------- |
| `datetime`   | 交易日期（`YYYY-MM-DD`）                                 |
| `instrument` | 股票代码                                               |
| `volume`     | 持仓数量（股）                                            |
| `cost_price` | 持仓成本价（复权调整后的当前成本，非原始买入价）                           |
| `price`      | 当日估值收盘价                                            |
| `weight`     | 权重（= 持仓市值 / 当日总资产，含成本口径，日终估值后计算）                   |
| `count_day`  | 连续持仓天数：买入成交日记 1，之后每持有一个交易日 +1；清仓后重新买入重置为 1；部分卖出不重置 |

- 停牌或已无行情（退市）的持仓照常输出一行：`price` 沿用最近一个有效收盘价，`count_day` 照常 +1（持仓仍在）。
- 不输出 CASH 行：各日 `weight` 之和 ≤ 1，差额即为当日现金占比。

### trades.csv —— 成交记录输出

`export_trades` 的输出格式，逐订单一行（含未成交订单，`deal_volume = 0`）：

| 字段            | 含义                  |
| ------------- | ------------------- |
| `datetime`    | 交易日期（`YYYY-MM-DD`） |
| `instrument`  | 股票代码               |
| `side`        | 方向：`buy` / `sell`  |
| `volume`      | 委托数量（股，绝对值）        |
| `price`       | 委托价格               |
| `deal_volume` | 实际成交数量（股，绝对值）      |
| `deal_price`  | 实际成交价（含滑点）         |
| `deal_cost`   | 实际交易费用             |

## 指标定义（Report 附录）

记组合逐日总资产为 `V_t`（含现金），逐日收益率 `r_t = V_t / V_{t−1} − 1`，基准逐日收益率 `b_t`，年化交易日数 `N = 252`，回测区间交易日数 `n`。

### export_data() 输出（report_data.csv）

`export_data()` 输出逐 bar 原始指标（字段参照仓库内 `report_data.csv`，**该文件仅作格式参考**；其日期写法 `2022/9/30` 为导出样本，实际输出统一为 `YYYY-MM-DD`）：

| 字段               | 含义                            |
| ---------------- | ----------------------------- |
| `datetime`       | 交易日期（`YYYY-MM-DD`）            |
| `account`        | 当日总资产（含成本口径，= `value + cash`） |
| `return`         | 当日收益率（含成本，首日约定为 0）            |
| `total_turnover` | 累计双边成交金额（元）                   |
| `turnover`       | 当日双边换手率（分母为前一交易日总资产，见衍生指标表）   |
| `total_cost`     | 累计交易费用（元）                     |
| `cost`           | 当日费用率（= 当日费用 / 前一交易日总资产）      |
| `value`          | 当日持仓市值                        |
| `cash`           | 当日现金                          |
| `benchmark`      | 所选基准当日收益率                     |

超额收益、年化、夏普等衍生指标由 Report 层基于上表计算，不在 `export_data` 中输出。

### 衍生指标公式

| 指标              | 公式                                                                                                                           |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 日收益率 `return`   | `r_t = V_t / V_{t−1} − 1`（首日 `r_0 = 0`，见补充定义"首日口径"）                                                                          |
| 基准日收益率 `bench`  | benchmark.csv 当日值                                                                                                            |
| 超额日收益率 `excess` | 默认 `r_t − b_t`（口径可配置，见下方说明）；不含成本与含成本各一列：`excess_without_cost` / `excess_with_cost`                                           |
| 累计净值            | `∏(1 + r_t)`，期初为 1                                                                                                           |
| 年化收益率           | `(∏(1 + r_t))^(N/n) − 1`；超额年化同理基于超额日收益复利                                                                                     |
| 年化波动率           | `std(r_t) × √N`（总体标准差，ddof = 0）                                                                                              |
| 夏普比率            | `mean(r_t) / std(r_t) × √N`（无风险利率取 0，ddof = 0）                                                                               |
| 最大回撤            | `max_t(1 − 净值_t / max_{s≤t} 净值_s)`                                                                                           |
| 信息比率            | `mean(excess_t) / std(excess_t) × √N`（ddof = 0）                                                                              |
| 换手率 `turnover`  | 双边换手 = `（当日买入成交金额 + 当日卖出成交金额）/ V_{t−1}`；成交金额为含滑点的实际成交口径（`deal_price × deal_volume`），分母为含成本口径前一交易日总资产（首个有成交的交易日用期初资金）；无成交日记 0 |
| 成本率             | `当日交易费用 / V_{t−1}`（= export_data 的 `cost` 列，分母为含成本口径前一交易日总资产）；无成交日记 0                                                        |

补充定义：

- **净值口径**：上表公式默认基于**含成本**收益序列 `r_t`（年化、波动、夏普、回撤同此）；如需不含成本口径，将 `r_t` 替换为 `r'_t` 同式计算。

- **首日口径**：首日 `r_0 = 0` 而 `b_0` 为基准当日实际收益，故 `excess_0 = −b_0`，不做特殊处理。注意副作用：若首个交易日即发生交易，当日盈亏（含首日费用）**不进入收益率序列，也不进入净值序列**（净值期初恒为 1），仅体现在 `export_data` 的 `account` 列中，并通过 `V_0` 的水平影响后续收益率的分母。年化公式中的 `n` 为区间交易日数（含首日），`r_0 = 0` 占用一天，属约定口径。

- **不含成本口径**：`return_without_cost` 基于费用扣减前的资产快照计算——账户需同时维护两条资产序列：含成本 `V_t`（费用扣现金）与不含成本 `V'_t`（费用不扣），`r'_t = V'_t / V'_{t−1} − 1`，`excess_without_cost = r'_t − b_t`。**该口径为近似**：`V'_t` 由 `V_t` 加回累计费用得到，忽略费用通过资金约束反解对成交股数的二阶影响（严格口径需重放一遍无费用回测），误差为费率量级，可接受。

- **换手率**：只输出双边口径（见上表 `turnover`）。

- **超额收益口径**：由 `gen_report` 的参数 `excess_method: "arithmetic" | "geometric"` 配置（便捷入口 `gen_report_default` 取 `"arithmetic"`，为算术差 `r − b`）；`"geometric"` 为几何差 `(1+r)/(1+b) − 1`。

---

## 非功能需求

### 信号质量评估（本期不做）

IC / RankIC 分析属于离线信号评估，利用 `pred.csv` 的 `ret` 列。本期不实现，仅在加载层做好 `ret` 的隔离，为后续模块预留。

> 可在后期提供最小 IC 计算工具。本期不做。

### 测试与验收

- **单元测试**：涨跌停判定、滑点计算、费用与 min_cost 反解、整手取整（含 SH688/SH689 的 200 股规则）、factor 复权调整（含当日新买入不调整、停牌恢复补调）、期初建仓（空仓首日买入 `top_n`）、除权日卖出按调整后 volume、停牌估值沿用、`deal_price` 列无效（缺失 / NaN / ≤ 0）不可交易。缺失类分支（`pre_close` / `high_limit` / `low_limit` / `factor` / `deal_price` 列缺失或非法）在真实数据中未必出现，须以**合成数据**构造用例覆盖。
- **合成用例精确验收（正确性基准）**：构造可手算的短用例（3~5 只股票、约 10 个交易日的小型 `stock_bar` / `pred` / `benchmark`），价格取整数或有限小数以消除浮点歧义，覆盖：涨跌停拦截、停牌、除权（factor 变化）、退市（行情终止）估值沿用、min_cost 触发、整手取整不足一手、资金约束反解、两阶段撮合核减买单。断言：
  - 逐笔成交明细（标的、方向、数量、成交价、费用）与手算值**完全一致**（经 `export_trades` 输出核对）；
  - 逐日账户序列（account / value / cash）与手算值**完全一致**；
  - 不变量：正常流程持仓只数 ≤ `top_n`；卖出委托量截断至持仓量；当日买入不可当日卖出（T+1）；进度条开关不改变逐日账户序列与成交记录（`elapsed` 除外）。
- **端到端冒烟测试**：用仓库内数据（`tmp_data/`，**仅作格式与规模参考**）跑通完整回测：全区间无报错、无 panic，三个输出文件（hist_position / trades / report_data）的列名与日期格式（`YYYY-MM-DD`）符合"数据文件格式"约定。**不做数值对拍**：仓库内 `report_data.csv` / `hist_position.csv` 不作为正确性基准。
- 若后续提供权威参考输出（真实数据 + 完整参数表），可追加大规模回归：逐日总资产相对误差 ≤ 1e-6（容忍浮点累乘顺序差异），成交价绝对误差 ≤ 1e-9，逐笔数量完全一致。

### 性能

- 目标：全量 A 股（约 5000 只 × 6 年日频）单次回测在分钟级完成；数据处理优先使用 polars 向量化，避免逐行 for 循环。
- 效率手段：将 `instrument` 转为 int 编码（`code`，见"代码规范"），加速 join / 分组 / 哈希与比较。
