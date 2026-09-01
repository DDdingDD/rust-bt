# Rust 股票回测系统 —— 系统架构设计

> 依据 `doc/specification.md`（下称"规范"）规划。本文档定义模块划分、核心类型、数据流与关键设计决策，作为实现阶段的顶层指导；接口签名为设计示意，实现时以规范为准。

---

## 1. 设计目标与原则

| 原则 | 说明 |
| --- | --- |
| 正确性优先 | 时间对齐（T 日信号 T+1 执行）、防前视、复权调整等正确性规则在类型与数据流层面强制，而非依赖约定 |
| 信息边界编译期化 | `Signal` 加载即剥离 `ret`；策略可见信息由 `StrategyContext` 的字段集合限定，无法越界访问 |
| 双层数据表示 | polars DataFrame 负责 IO / 校验 / 报表；主循环使用按日切片的 SoA（列式 Vec）结构，避免逐行访问 DataFrame |
| 日内索引贯穿 | 交易日历索引（`DayIdx`）与股票 int 编码（`code: u32`）作为内部主键，字符串日期 / `instrument` 仅存在于 IO 边界 |
| 单 crate 多模块 | 规模适中，暂不拆 workspace；模块边界按可拆分设计，预留演进空间 |

---

## 2. 总体分层

```text
┌─────────────────────────────────────────────┐
│ src/bin/bt.rs（CLI：bt <config.yml>）        │
│ examples/run_backtest.rs（规范"使用方法"示例） │
├─────────────────────────────────────────────┤
│ 嵌入 API api：run / BtParams / BtOutput      │
│ （高层便捷层：装配+回测+报告折叠为一个入口，    │
│   参数类型化；CLI 组装复用此层）              │
├─────────────────────────────────────────────┤
│ Facade：load_signal / BTData / Backtest /    │
│         BTResult / Report（规范公开接口）      │
│ config：BtConfig（YAML 反序列化 + 默认值 +    │
│         必填校验，经 to_params 转嵌入 API     │
│         参数，供 CLI 组装）                   │
├──────────────┬──────────────┬───────────────┤
│ 编排层        │ 领域层        │ 报表层         │
│ backtest     │ strategy      │ report        │
│ （主循环、    │ exchange      │ （指标、导出、  │
│  两阶段撮合） │ account       │  绘图）        │
│              │ position/order│               │
├──────────────┴──────────────┴───────────────┤
│ 数据层 data：加载、校验、交易日历、int 编码、    │
│             按日索引存储                        │
├─────────────────────────────────────────────┤
│ 基础层 types / error                         │
└─────────────────────────────────────────────┘
```

依赖方向自上而下，禁止反向依赖：`report` 不依赖 `backtest`；`strategy` / `exchange` 互不依赖（共用的 `TradableInfo` / `StockTradable` 类型下沉至基础层 `types`，见 §4.1/§4.6），均由 `backtest` 编排。

---

## 3. 文件与模块划分

```text
rust-bt/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # 公开 re-export（load_signal, Signal, BTData, Account, Exchange,
│   │                           #  Backtest, Strategy, TopkDropoutStrategy, TopkStrategy, BTResult,
│   │                           #  Report, Decision, Order 等实现自定义策略所需类型；api 层 run/BtParams 等）
│   ├── error.rs                # BtError（thiserror 类型化错误，见 §8）
│   ├── types.rs                # DayIdx、DealPrice、ExcessMethod、BenchmarkName、instrument 编解码、
│   │                           #  TradableInfo/StockTradable（strategy 与 exchange 共用，见 §4.6）
│   ├── data/
│   │   ├── mod.rs              # BTData 构建器（new/load_stock_bar/load_benchmark/build）+
│   │   │                       #  read_dataframe（CSV/parquet 按扩展名分发）、date_strings（类型化日期统一转 YYYY-MM-DD）
│   │   ├── calendar.rs         # TradingCalendar：交易日序列、区间对齐与校验
│   │   ├── stock_bar.rs        # stock_bar 加载、结构校验、StockBarStore（排序帧 + 日偏移索引）
│   │   ├── wap.rs              # wap 时段数据加载、校验、WapStore（vwapN/twapN 方向价/量）
│   │   └── benchmark.rs        # benchmark 加载与校验（重复键、结构）
│   ├── signal.rs               # Signal：pred 加载、结构校验、剥离 ret、按日索引
│   ├── order.rs                # Order、Decision、TradeRecord
│   ├── position.rs             # PositionEntry、Position 容器、factor 复权调整
│   ├── account.rs              # Account：记账、双资产口径、日终估值、逐日记录
│   ├── exchange/
│   │   ├── mod.rs              # Exchange：参数、行情注入、deal_order 撮合入口
│   │   ├── market.rs           # DailyMarketStore：limit 预计算、可交易性、按日 SoA 视图
│   │   └── rules.rs            # 纯函数规则：滑点、费用与 min_cost 反解、整手取整（含科创板）
│   ├── strategy/
│   │   ├── mod.rs              # Strategy trait、StrategyContext、PostSellContext
│   │   ├── common.rs           # 可复用构件：排名（同分字典序）、等权资金分配、金额->股数换算
│   │   ├── topk_dropout.rs     # TopkDropoutStrategy
│   │   └── topk.rs             # TopkStrategy（每日持有 score 前 top_n 只，跌出才卖出）
│   ├── backtest.rs             # Backtest：主循环、两阶段撮合编排、延期校验、进度条与耗时
│   ├── result.rs               # BTResult：hist_position / trades 导出、gen_report
│   ├── api.rs                  # 嵌入 API（§4.10）：run / BtParams / StrategySpec / ExchangeParams /
│   │                           #  BtOutput（export_all）/ signal_from_pairs 便捷构造
│   ├── config.rs               # BtConfig：YAML 配置（serde 默认值 + 必填校验），经 to_params 供 CLI
│   ├── bin/
│   │   └── bt.rs               # CLI 入口（bt <config.yml>）：加载配置与信号 -> api::run -> export_all
│   └── report/
│       ├── mod.rs              # Report：PortfolioMetrics、export_data、衍生指标
│       └── html.rs             # HTML 报告：指标表 + 7 面板图（plotly CDN，路径由调用方指定）
├── config.example.yml          # CLI 配置示例（全字段注释，默认值与 config.rs 一致）
├── examples/
│   └── run_backtest.rs         # 与规范"使用方法"一致的端到端示例
└── tests/
    ├── acceptance/             # 合成用例精确验收（手算对拍）
    │   ├── mod.rs
    │   └── cases/              # 每用例一个文件：limit、paused、factor、delist、min_cost、lot、cash、两阶段核减、期初建仓、deal_price 列无效
    ├── acceptance_wap.rs       # WAP 时段价合成验收（方向价、方向量、缺失行、策略可见价）
    ├── data/synthetic/         # 合成 CSV（3~5 只股票、约 10 个交易日）
    ├── smoke_tmp_data.rs       # tmp_data 端到端冒烟（数据缺失时自动跳过）
    └── smoke_wap_data.rs       # tmp_data/wap.parquet 时段价冒烟
```

---

## 4. 核心类型设计

### 4.1 基础类型（types.rs）

```rust
/// 交易日历索引：内部时间主键，0..n_days
pub type DayIdx = u32;
/// 股票 int 编码：SH600000 -> 600000（见规范"代码规范"）
pub type Code = u32;

pub enum DealPrice { Open, Close, Vwap, Wap { kind: WapKind, window: u8 } } // TryFrom<&str>；Wap = vwapN/twapN（N=1..=11）
pub enum WapKind { Vwap, Twap }
impl WapKind { pub fn parse(s: &str) -> Result<Self>; pub fn as_str(&self) -> &'static str; }
pub enum ExcessMethod { Arithmetic, Geometric } // TryFrom<&str>，非法值 Err
pub enum BenchmarkName { Hs300, Zz500, Cyb, Zz800, Zz1000, Zz2000, Sci, Kci, Cyi }
impl BenchmarkName {
    pub fn from_name(s: &str) -> Option<Self>;   // 名称不在映射表 -> None -> Err
    pub fn instrument(&self) -> &'static str;    // -> 数据文件中的指数代码
}

// instrument <-> code 编解码（仅在 IO 边界使用）
pub fn parse_instrument(s: &str) -> Result<Code>;   // "SH600000" -> 600000；无法解析报错
pub fn format_instrument(code: Code) -> Result<String>; // 600000 -> "SH600000"
```

反向映射规则：数字段首位 `6` → `SH`；`0` / `3` → `SZ`；其余（`4`/`8`/`9`，北交所段）当前报错，接入北交所时扩展，编码规则不变。

`types.rs` 同时承载 `TradableInfo` / `StockTradable`（定义见 §4.6）：作为 `strategy`（决策可见）与 `exchange`（market.rs 构建）的共用类型置于基础层，两模块因此互不依赖（§2）。

### 4.2 数据层

```rust
/// 交易日历：升序日期 + 日期->DayIdx 反查
pub struct TradingCalendar { dates: Vec<NaiveDate>, index: HashMap<NaiveDate, DayIdx> }
impl TradingCalendar {
    /// 闭开区间 [start, end) 对齐；start >= end 或区间内无交易日 -> Err
    pub fn align(&self, start: &str, end: &str) -> Result<Range<DayIdx>>;
    pub fn date(&self, idx: DayIdx) -> NaiveDate;
    pub fn contains(&self, d: NaiveDate) -> bool;
}

/// stock_bar 存储：按 (DayIdx, Code) 排序的 DataFrame + 每日行范围索引
pub struct StockBarStore {
    frame: DataFrame,                    // code: u32 已编码，列含 open/close/.../factor/paused/is_st/vwap
    day_offsets: Vec<(u32, u32)>,        // 每个交易日 (start_row, len)，O(1) 取当日切片
}

pub struct BTData { /* stock_bar: Option<StockBarStore>, wap: Option<WapStore>, benchmark: Option<DataFrame>, calendar: TradingCalendar */ }
impl BTData {
    pub fn new() -> Self;
    pub fn load_stock_bar(self, path: &str) -> Result<Self>;   // 结构校验（见规范校验表）；CSV/parquet 按扩展名识别
    pub fn load_wap(self, path: &str, window: u8) -> Result<Self>; // vwapN/twapN 时段数据；parquet 按列投影只读 6 列
    pub fn load_benchmark(self, path: &str) -> Result<Self>;
    pub fn build(self) -> Result<Self>;                        // 统一校验 + 构建交易日历 + 日索引
}
```

- 日历 = stock_bar 去重排序后的全部 `datetime`（规范"交易日历"）。
- 加载边界完成：`instrument` → `code` 编码、必需列存在性、重复键、价格/factor 合法性校验。`paused`/`is_st` 缺失按 0 处理并 warning。非错误类行级规则（停牌、无量、deal_price 列无效）不在加载期剔除，而是体现在撮合期可交易性判断。
- 输入格式：`data::read_dataframe` 按扩展名分发（`.parquet`/`.pq` -> ParquetReader，其余 -> CSV），两种格式共用同一套列校验；parquet 的类型化 `datetime`（`Date`/`Datetime`）经 `data::date_strings` 统一截断到日、转 `YYYY-MM-DD` 字符串（规范"数据文件格式"）。

### 4.3 Signal

```rust
pub struct Signal {
    days: BTreeMap<NaiveDate, SignalDay>,   // ret 列已在加载时剥离，结构上不存在
}
pub struct SignalDay { pub codes: Vec<Code>, pub scores: Vec<f64> }  // 按 score 无序，策略自排序

pub fn load_signal(path: &str) -> Result<Signal>;

// 内存构造（嵌入方程序化生成信号；校验口径同 load_signal）
impl SignalDay { pub fn from_pairs(pairs: Vec<(String, f64)>) -> Result<SignalDay>; }
impl Signal {
    pub fn from_days(days: BTreeMap<NaiveDate, SignalDay>) -> Signal;
    pub fn dates(&self) -> impl Iterator<Item = NaiveDate> + '_;
}
// polars DataFrame 直连：load_signal = 读 CSV + 本函数（校验单点维护）
pub fn signal_from_dataframe(df: &DataFrame) -> Result<Signal>;
```

- `load_signal` 只做结构校验（重复键、score 缺失/NaN 丢弃 + warning）并剥离 `ret`。
- `from_pairs` 口径一致：同日 instrument 重复报错；不可解析 / NaN score 丢弃 + warning。
- `signal_from_dataframe` 与 `load_signal` 共用同一实现：必需 `datetime` / `instrument` /
  `score` 列（cast 为 String/String/f64），多余列（含 `ret`）忽略剥离。
- 日历/行情相关校验（datetime 不在日历、instrument 无行情）推迟到 `Backtest::run` 启动时执行（规范"数据校验"两阶段）。
- 按日期建索引，主循环取 T−1 日信号为 O(log n) / O(1)。

### 4.4 Order / Decision

```rust
pub struct Order {
    pub stock: Code,
    pub volume: f64,        // 买正卖负
    pub price: f64,         // 委托价（策略填，取 T_exec 日 deal_price 列）
    pub deal_volume: f64,   // 以下由 Exchange 回填
    pub deal_price: f64,
    pub deal_cost: f64,
}

/// 策略单步输出。分卖/买两组是规范"先卖后买"与两阶段撮合的直接表达。
pub struct Decision {
    pub sell_orders: Vec<Order>,
    /// 按优先级降序排列（如 score 降序，分数高的优先获得资金；核减时从尾部丢弃）
    pub buy_orders: Vec<Order>,
    /// 默认核减钩子（Strategy::revise_buy_orders）的输入：卖出成交后，
    /// 买单只数核减至 target − 实际持仓数。None 表示默认不核减。
    /// TopkDropout 置 Some(top_n)；自定义核减语义的策略覆写钩子后可忽略本字段。
    pub target_positions: Option<usize>,
}
```

`TradeRecord` = Order 的导出投影 + `datetime` / `side`（trades.csv 行）。

### 4.5 Position / Account

```rust
pub struct PositionEntry {
    pub volume: f64,
    pub cost_price: f64,     // 记账成本（复权调整后）
    pub price: f64,          // 最近有效收盘价（估值用）
    pub last_factor: f64,    // 最近一次入账时的复权因子
    pub count_day: u32,      // 连续持仓天数
}

pub struct Account {
    cash: f64,                              // 允许为负（卖出费用超成交金额情形）
    positions: HashMap<Code, PositionEntry>,
    cum_cost: f64,                          // 累计费用：不含成本口径 V' = V + cum_cost
    daily: Vec<DailyRecord>,                // PortfolioMetrics 源数据（逐日 push）
    hist_positions: Vec<HistPositionRow>,   // 逐日持仓快照（导出 hist_position 用）
}
impl Account {
    pub fn new(cash: f64) -> Self;          // 期初全现金；期初资金随 BTResult 导出（initial_cash，§4.9）
}
struct DailyRecord {
    day: DayIdx, account: f64, value: f64, cash: f64,
    turnover_amount: f64,                   // 当日双边成交金额 Σ(deal_price × deal_volume)，含滑点口径
    cost: f64,                              // 当日交易费用
}
```

账户职责（对应规范"回测主循环时序"第 2/4/5 步）：

- `adjust_factor(day_market)`：撮合前，对当日 `factor ≠ last_factor`（epsilon 比较）的持仓做 `volume ×= ratio; cost_price /= ratio; last_factor 更新`。当日新买入尚未入账，天然不参与。
- `on_deal(order, day)`：成交落账——现金增减（含费用）、`PositionEntry` 更新（买入 `cost_price` = 含滑点成交价，费用不摊入；`last_factor` 记当日 factor；`count_day` 新买入记 1）、累加当日 `turnover_amount` 与 `cost`。
- `end_of_day(day_market, day)`：估值——有有效行情（未停牌且 close > 0）的持仓更新 `price`，停牌/退市沿用；`account = cash + Σ(volume × price)`；`count_day +1`（当日新买入已在 `on_deal` 记 1，不重复）；push `DailyRecord` 与持仓快照。
- 双资产口径：`V'_t = V_t + cum_cost_t` 在 Report 层由 `DailyRecord.account + 累计 cost` 派生，账户无需维护第二条完整序列，只需逐日记录 `cost`。

### 4.6 Strategy

```rust
/// 策略在 T_exec 日可见的全部信息（规范"信息边界"的编译期落实）
pub struct StrategyContext<'a> {
    pub signal: &'a SignalDay,          // T−1 日信号；无信号日 Backtest 直接跳过决策（§4.8 主循环
                                        // a 步，引擎级保证），gen_decision 不会被以"无信号"状态调用
    pub positions: &'a HashMap<Code, PositionEntry>,  // 复权调整后的当前持仓
    pub cash: f64,
    pub tradable: &'a TradableInfo,     // T_exec 日可交易性
    pub day: DayIdx,
}

// TradableInfo / StockTradable 定义于 types.rs（§4.1）：strategy（决策可见）与 exchange
// （market.rs 构建）共用同一类型，两模块互不依赖、口径一致（§2、D4）
pub struct StockTradable {
    pub suspended: bool,     // paused = 1 或 close 缺失
    pub limit_buy: bool,
    pub limit_sell: bool,
    pub volume_cap: f64,     // 买入侧成交量上限：volume × volume_threshold（wap 模式 = buy_volume × threshold）；threshold = None 时 f64::INFINITY
    pub sell_volume_cap: f64, // 卖出侧成交量上限（普通模式与 volume_cap 同值）
    pub deal_price: f64,     // T_exec 日策略可见价：普通模式 = deal_price 列；wap 模式 = pre_close
    pub is_st: bool,        // 规范 forbid_st 参数所需；ST 状态盘前公开、非价格信息，不属前视
}
pub struct TradableInfo { /* 当日 SoA + Code -> 行索引 */ }
impl TradableInfo { pub fn get(&self, code: Code) -> Option<StockTradable>; } // None = 当日无行情

pub trait Strategy {
    fn gen_decision(&mut self, ctx: &StrategyContext) -> Result<Decision>;

    /// 阶段一（卖出全部撮合完成）之后、阶段二（买入撮合）之前的买单修正钩子。
    /// 默认实现：按 Decision.target_positions 截断买单（None 则原样返回）——
    /// 即 TopkDropout / Topk 的核减语义，两者均无需覆写。
    /// 需要其他语义的策略（如"按卖出实际回款重新分配买单金额"）覆写本方法；
    /// 可见信息均为 T_exec 日合法信息（卖出成交结果当日可得、回款当日可用），不破坏信息边界。
    fn revise_buy_orders(
        &self,
        buys: Vec<Order>,
        after_sell: &PostSellContext,
    ) -> Result<Vec<Order>> { /* 默认实现：target_positions 截断 */ }
}

/// 核减钩子可见的卖出后状态
pub struct PostSellContext<'a> {
    pub positions: &'a HashMap<Code, PositionEntry>, // 卖出成交后的实际持仓
    pub cash: f64,                                   // 含卖出回款（已扣费用）
    pub tradable: &'a TradableInfo,                  // 与决策时同一份当日视图
    pub filled_sells: &'a [Order],                   // 卖单成交结果（含部分成交/未成交回填）
}
```

`TopkDropoutStrategy { top_n, drop_n, only_tradable, forbid_st }`（构造入口 `new(top_n, drop_n)`：`only_tradable` / `forbid_st` 默认 `false`，builder 方法覆写）：按规范"内置策略"实现排名、卖出、资金分配（毛额口径预期回款、一次性确定金额不重新分配），`Decision.target_positions = Some(top_n)`，买单按 score 降序、同分按 code 升序（字典序确定性），核减使用 trait 默认钩子。排名、同分比较、等权分配、金额→股数换算等通用逻辑实现于 `strategy/common.rs`，供后续新策略复用。无信号日不调仓由引擎保证（§4.8 主循环 a 步），策略无需处理。

`top_n` 与 `drop_n` 为独立参数，不要求相等：`top_n=100, drop_n=50` 即"持有 100 只、每日轮换最差的 50 只"的半仓轮动用法，`n_buy = top_n − 保留持仓数` 公式与两阶段核减天然兼容。构造期校验 `top_n ≥ 1`、`drop_n ≥ 0`；`drop_n > top_n` 不报错但 warning（退化为每日清空排名内持仓再重建，行为确定但通常非预期）。

**历史信号窗口（演进预留，本期不实现）**：动量/均线类策略需要过去 N 日信号时，由 Backtest 统一切窗，在 `StrategyContext` 增加 `signal_window: &[&SignalDay]`（T−1 往前、长度可配），**不向策略暴露完整 `Signal`**（信号文件含 T 日及之后的数据，整体暴露即前视）。切窗点收敛在 Backtest，保证边界不可越过。

### 4.7 Exchange

```rust
pub struct Exchange {
    config: ExchangeConfig,       // deal_price、费率、min_cost、滑点、volume_threshold、limit_threshold
    market: DailyMarketStore,     // Backtest::new 时注入；limit_buy/limit_sell 已按 deal_price 预计算
}
impl Exchange {
    /// 构造期校验：deal_price 合法；limit_threshold ∈ (0, 0.1]，越界 Err（InvalidParam）；
    /// None -> 不做涨跌停限制并 warning（涨跌停的股票也能被买卖）
    pub fn new(deal_price: &str, open_cost: f64, close_cost: f64, min_cost: f64,
               fixed_slippage: f64, min_slippage_ratio: f64,
               volume_threshold: Option<f64>, limit_threshold: Option<f64>) -> Result<Self>;
    /// 单订单撮合：可交易性 -> 裁剪 -> 滑点 -> 资金反解 -> 整手 -> 费用 -> 回填 -> account.on_deal
    /// wap 模式：撮合基价与量上限按方向取 wap_N_*_buy / _sell 与 buy_volume / sell_volume
    pub fn deal_order(&self, order: &mut Order, account: &mut Account, day: DayIdx);
}
```

`deal_order` 单订单流水线（与规范"撮合通用规则"逐条对应）：

```text
1. 当日无行情               -> deal_volume = 0（退市不可交易）
2. 停牌检查（先行！）        -> 0
3. 涨跌停检查               -> 买单 limit_buy / 卖单 limit_sell -> 0
4. 成交量裁剪               -> min(|volume|, volume_cap)；当日无量 -> 0
5. 卖出截断至持仓量（warning）
6. 滑点                     -> adj_ratio = max(min_slippage_ratio, fixed/price)，按方向调整，不 clamp
7. 买单资金反解（两 regime，取可行解较大者；用滑点后价格；现金连 min_cost 都不足 -> 0）
8. 整手取整                 -> 买入 100 股；SH688/SH689 按 1 股取整、不足 200 股归零；卖出不取整
9. 费用                     -> max(val × ratio, min_cost)；volume = 0 时费用 0
10. 回填 + account.on_deal
```

`rules.rs` 中滑点 / 费用 / 反解 / 取整均为**纯函数**，是单元测试的主要标的。`market.rs` 在注入时预计算 `limit_buy` / `limit_sell`（缺失分支按规范区分：`pre_close` 缺失如上市首日属常态，仅置 false 不告警；`high_limit` / `low_limit` 自身缺失才置 false 并 warning），并提供按日 SoA 视图与 `TradableInfo` 构建（Exchange 与 Strategy 共用同一份当日数据，保证口径一致）。

### 4.8 Backtest

```rust
pub struct Backtest { data: BTData, account: Account, exchange: Exchange, strategy: Box<dyn Strategy>,
                      initial_cash: f64 /* 账户构造时的期初现金；装配 BTResult 时导出（§4.9 报表首日分母） */,
                      progress: bool /* 终端进度条开关，默认 false（D12） */,
                      has_run: bool /* run 单次性守卫：首次运行消耗账户 / 基准 / 逐日记录（take 语义），二次调用 Err */ }
impl Backtest {
    pub fn new(data: BTData, account: Account, exchange: Exchange, strategy: Box<dyn Strategy>) -> Self;
    // new 内完成：exchange 注入行情（按 deal_price 预计算 limit 列）
    /// 进度条开关（默认关闭）：启用后 run 期间向 stderr 渲染按交易日推进的进度条，
    /// 总数 = 对齐区间交易日数（启动校验后确定），结束行显示总耗时（D12）
    pub fn with_progress(self, enabled: bool) -> Self;
    /// 只能调用一次；二次调用返回 Err（InvalidParam），再次回测需重新装配
    pub fn run(&mut self, signal: &Signal, start_date: &str, end_date: &str) -> Result<BTResult>;
}
```

`run` 主循环：

```text
0. 启动校验：日历区间对齐（Err 规则见规范）；
   信号延期校验（datetime 不在日历 / instrument 无行情 -> 丢弃 + warning）
   计时与进度：Instant::now() 记起点；progress = true 时创建进度条（stderr，总数 = 区间交易日数）
1. for day in 对齐区间:
   a. 取 T−1 日 SignalDay；无 -> 本日不产生任何订单（引擎级保证，规范主循环第 1 步），
      跳过 c~f，仅执行 b 与 g（复权与估值同当日是否交易无关）
   b. account.adjust_factor(day_market)               // 撮合前复权
   c. decision = strategy.gen_decision(ctx)           // 仅在 a 取到信号时调用
      - Decision 合法性：同股同时买+卖 -> Err；同股多笔买单合并为一笔
        （多笔卖单同理合并，规范仅约定买单，此处对称钉死）
   d. 阶段一：逐单 deal_order(sell)                   // 卖单全部撮合，回款当日可用
      每笔订单（含 deal_volume = 0 的未成交单）追加到 trades 日志
   e. 核减：buy_orders = strategy.revise_buy_orders(decision.buy_orders, post_sell_ctx)
      // 默认实现按 target_positions 截断；被核减丢弃的买单未进入撮合，不产生 trades 行
   f. 阶段二：按序逐单 deal_order(buy)                // 优先级高者优先获得资金
      每笔订单（含 deal_volume = 0 的未成交单）追加到 trades 日志
   g. account.end_of_day(day_market, day)             // 估值 + 逐日记录 + 持仓快照
   h. progress.inc(1)                                 // 每交易日一次；禁用时为 None 零开销
2. 装配 BTResult（elapsed = run 全程墙钟耗时；进度条 finish 显示总耗时）
```

### 4.9 BTResult / Report

```rust
pub struct BTResult {
    daily: Vec<DailyRecord>,            // 逐日账户序列
    hist_positions: Vec<HistPositionRow>,
    trades: Vec<TradeRecord>,
    calendar: TradingCalendar, range: Range<DayIdx>,
    benchmark: DataFrame,               // 多指数原始帧，gen_report 时选定
    initial_cash: f64,                  // 期初资金：turnover / cost 在区间首个有成交交易日的分母
    elapsed: Duration,                  // run() 墙钟耗时：含启动校验与结果装配，不含 BTData 加载
}
impl BTResult {
    pub fn export_hist_position(&self, path: &str) -> Result<()>;  // weight = 市值/当日总资产，导出时计算
    pub fn export_trades(&self, path: &str) -> Result<()>;
    pub fn gen_report(&self, benchmark: &str, excess_method: &str) -> Result<Report>;
    pub fn gen_report_default(&self) -> Result<Report>; // = gen_report("zz1000", "arithmetic")
    pub fn elapsed(&self) -> Duration;                   // run() 耗时（元数据，不进导出文件与报表）
}

pub struct Report { metrics: DataFrame /* export_data 逐 bar 表 */, derived: DerivedStats, /* + 绘图序列（含/不含成本净值、回撤、超额、换手率） */ }
impl Report {
    pub fn export_data(&self, path: &str) -> Result<()>;
    pub fn plot(&self, path: &str) -> Result<()>;   // HTML（调用方指定路径）：指标表 + 7 面板图，见 D10
    pub fn summary(&self) -> String;                // 简报：关键指标文本（CLI 尾部输出 / 嵌入方日志）
}
```

`gen_report` 流程：基准名 → 映射表（不在表内 Err）→ 过滤该指数、剔除日历外行 → **覆盖校验**（必须覆盖回测全部交易日，否则 Err；区间内 benchmark 值缺失/NaN/inf 同样 Err）→ 与逐日记录 join → 计算 `return` / `turnover` / `cost` 等逐 bar 列（polars 向量化）→ 含/不含成本两条收益序列派生超额与累计净值 → 衍生指标（年化、波动、夏普、最大回撤、信息比率，ddof = 0）。导出边界完成 `code` → `instrument` 反映射与 `YYYY-MM-DD` 日期格式化。

首日口径（规范"指标定义--补充定义"）：`r_0 = 0` -- 首日盈亏（含首日费用）不进入收益率与净值序列（净值期初恒为 1），仅体现在 `account` 列，并通过 `V_0` 的水平影响后续收益率分母；`excess_0 = −b_0` 不做特殊处理；年化公式中的 `n` 为区间交易日数（含首日）。`turnover` / `cost` 的分母为前一交易日总资产，区间首个有成交的交易日无前日，取期初资金 `initial_cash`。

Report 另提供全部绘图序列的只读访问器（`dates` / `metrics` / `cum_with_cost` / `cum_without_cost` / `cum_benchmark` / `drawdown` / `drawdown_without` / `cum_excess` / `cum_excess_without` / `excess_drawdown` / `excess_drawdown_without` / `turnover`），供嵌入方程序化消费（不必落盘再读 CSV）。

### 4.10 嵌入 API（api.rs）

```rust
pub struct BtParams {
    pub stock_bar: String, pub benchmark: String, pub wap: Option<String>, // wap 时段数据路径：deal_price = vwapN/twapN 时必填
    pub start_date: String, pub end_date: String,
    pub initial_cash: f64,
    pub strategy: StrategySpec,                          // TopkDropout{..} / Topk{..} 参数化或 Custom(Box<dyn Strategy>)
    pub exchange: ExchangeParams,                        // deal_price: DealPrice 枚举，Default 对齐 CLI 默认
    pub benchmark_name: BenchmarkName, pub excess_method: ExcessMethod,
    pub progress: bool,
}
pub enum StrategySpec {
    TopkDropout { top_n: usize, drop_n: usize, only_tradable: bool, forbid_st: bool },
    Topk { top_n: usize, forbid_st: bool },
    Custom(Box<dyn Strategy>),
}
pub struct BtOutput { pub result: BTResult, pub report: Report }
impl BtOutput { pub fn export_all(&self, dir: impl AsRef<Path>, names: &ExportNames) -> Result<()>; }

pub fn run(params: BtParams, signal: &Signal) -> Result<BtOutput>;
pub fn run_from_signal_file(params: BtParams, signal_path: &str) -> Result<BtOutput>;
pub fn signal_from_pairs(days: BTreeMap<NaiveDate, Vec<(String, f64)>>) -> Result<Signal>;
```

`run` 内部流程与组件层等价：数值参数校验 -> 装配（`Exchange::new` 费用/阈值校验）-> 加载数据 -> 主循环 -> `gen_report`，校验先于数百 MB 行情加载（fail fast）。CLI（`bt.rs`）与示例共享该路径：`BtConfig::to_params()` 转类型化参数后调 `run`，消除双份装配逻辑。

---

## 5. 关键设计决策

| # | 决策 | 理由 |
| --- | --- | --- |
| D1 | 单 crate 多模块，不拆 workspace | 当前规模单 crate 编译更快、依赖简单；模块边界按 crate 标准设计，未来可平移 |
| D2 | 双层数据表示：polars 做 IO/校验/报表，主循环用按日 SoA 切片 | 规范性能目标（5000 股 × 6 年日频数据，单次回测分钟级完成）。逐行 DataFrame 访问开销大；加载后一次转换为 `day_offsets` 索引的列式 Vec，主循环零拷贝切片 |
| D3 | `Decision` 拆 sell/buy 两组；核减编排为 `Strategy::revise_buy_orders` 钩子（默认按 `target_positions` 截断） | 核减规则（top_n − 实际持仓）是策略知识，但两阶段编排（先卖、核减、后买）在 Backtest。钩子方案使 Backtest 无需 downcast 具体策略，也不把 TopkDropout 语义硬编码进编排层：TopkDropout 用默认实现零成本，新策略覆写钩子即可获得卖出成交结果并实现自定义核减（如按实际回款重新分配金额） |
| D4 | 信息边界编译期化：`Signal` 结构上无 `ret` 列；策略只拿到 `StrategyContext` | 防前视不靠注释约束：策略无法引用不存在的字段。`TradableInfo` 与 Exchange 撮合共用同一份当日视图，避免策略与撮合口径分叉 |
| D5 | 不含成本口径由 `V + 累计费用` 在 Report 层派生 | 规范明确该口径为可接受近似；账户只需逐日记 `cost`，无需双序列记账 |
| D6 | `instrument`/`code`、日期字符串/`DayIdx` 的转换全部收敛在 IO 边界 | 内部全程 u32/u32 主键，join/哈希/比较快；导出统一反映射，格式（`SH600000`、`YYYY-MM-DD`）单点控制 |
| D7 | 停牌检查先于涨跌停，且 limit 预计算在行情注入 Exchange 时按 deal_price 完成 | 规范明确判定顺序与预计算时机；注入点（`Backtest::new`）是唯一同时持有行情与 deal_price 参数的位置 |
| D8 | 撮合规则（滑点/费用/反解/整手/limit 判定）实现为纯函数模块 `rules.rs` | 规范的单元测试清单（涨跌停、min_cost 反解、科创板 200 股、滑点）全部落在纯函数上，可无 fixture 直测 |
| D9 | warning 统一走 `log` facade | 库不绑定具体 logger；示例/测试用 env_logger 初始化。warning 语义（丢弃信号、缺失按 0、截断卖出）散落在各层，统一通道便于审计 |
| D10 | 报告绘图：自包含 HTML（plotly.js basic bundle vendor 于 `assets/`、`include_str!` 内嵌，字符串模板手工拼 JSON）~~（原：plotters 直出 PNG，2026-08 取代）~~ | 交互式报告格式（7 面板：含/不含成本累计收益、两口径回撤、累计超额、换手率、两口径超额回撤 + 衍生指标表），可交互缩放/悬浮取值；手工拼 JSON 零新增 Rust 依赖、无前端构建；渲染为纯函数（`report/html.rs`）可无 fixture 直测。内嵌 basic bundle（仅 scatter/bar/pie，约 1.1MB）使单文件离线可开，代价为仓库/二进制/产物各 +1.1MB |
| D11 | 错误分层：内部 `thiserror` 类型化 `BtError`，公开 API 返回 `anyhow::Result` | 与规范示例签名（`anyhow::Result`）一致；内部保留错误分类（数据校验/日历/非法参数/撮合）便于测试断言与调用方 downcast |
| D12 | 进度条与耗时：`with_progress` 开关（默认关闭）+ indicatif 渲染 stderr；耗时记 `BTResult.elapsed`，进度条结束行显示 | 库默认零终端输出（测试、无终端、输出重定向环境干净）；总日数在区间对齐后即知，进度与 ETA 确定；禁用时 `Option<ProgressBar>` 为 None、`Instant` 计时开销可忽略，不触碰主循环热路径（每交易日一次 `inc`，重绘由 indicatif 节流） |
| D13 | 双层公开 API：高层便捷层 `api::run`（类型化 `BtParams` -> `BtOutput`）与组件 Facade 并存；CLI 经 `BtConfig::to_params` 复用高层层 | 嵌入方一次调用完成装配+回测+报告，参数用枚举（`DealPrice`/`BenchmarkName`/`ExcessMethod`）编译期杜绝拼写错误（YAML 层只能加载期校验）；信号支持内存构造（`SignalDay::from_pairs`，校验口径同 `load_signal`）；两层共用同一撮合与估值路径（集成测试对拍逐日账户与逐笔成交完全相等），CLI/示例/嵌入不再各维护一份装配逻辑。数据复用（参数扫描免重载行情）留待组件层后续演进（如 `Arc<StockBarStore>`） |
| D14 | WAP 时段价方向化：`deal_price = vwapN/twapN` 时，策略可见价取 `pre_close`（决策时点合法已知），撮合基价与量上限按方向取 wap 数据的 `_buy`/`_sell` 列；装配期校验 wap 时段与 deal_price 一致 | 支持日内固定窗口 VWAP/TWAP 回测，同时守住信息边界：策略不提前看到 wap 价格，交易所按方向真实价格与容量成交；方向量上限防止用全日容量高估单边成交 |

---

## 6. 错误与告警策略

```rust
#[derive(thiserror::Error, Debug)]
pub enum BtError {
    #[error("数据校验失败: {0}")] Validation(String),        // 重复键、缺列、非法价格/factor
    #[error("交易日历: {0}")] Calendar(String),              // 区间对齐失败
    #[error("非法参数: {0}")] InvalidParam(String),          // deal_price / benchmark / excess_method / limit_threshold 越界
    #[error("基准覆盖不足: {0}")] BenchmarkCoverage(String), // gen_report 覆盖校验
    #[error("决策非法: {0}")] InvalidDecision(String),       // 同股买卖冲突等
    #[error(transparent)] Polars(#[from] polars::error::PolarsError),
    #[error(transparent)] Io(#[from] std::io::Error),
}
```

- 硬规则（规范"报错"项）→ `Err(BtError)`，不 panic；字符串枚举参数在入口 `TryFrom` 处拒绝。
- 软规则（规范"warning"项）→ `log::warn!` + 文档化处理（丢弃/置 0/置 false），不中断流程。
- 内部不变量（如排序帧与 day_offsets 一致性）用 `debug_assert!` 防护，不作为错误处理路径。

---

## 7. 依赖选型

| crate | 用途 |
| --- | --- |
| polars（features: csv, parquet, temporal） | 数据加载（CSV/parquet）、校验、join、报表指标向量化计算；temporal 支撑 parquet 类型化日期列转换 |
| chrono | `NaiveDate`，`YYYY-MM-DD` 解析/格式化 |
| thiserror / anyhow | 错误分层（见 D11） |
| log | warning 通道（D9） |
| indicatif | 终端进度条：stderr 渲染、按时间节流重绘（D12） |
| env_logger | CLI 与示例/测试的日志初始化 |
| serde / serde_yaml | `config.rs` 的 YAML 配置反序列化（bt CLI） |

---

## 8. 测试架构

对应规范"测试与验收"三层：

1. **单元测试**（模块内 `#[cfg(test)]`）：`rules.rs` 纯函数全覆盖——limit 判定（含 5%/10%/20% 板幅与容差比例）、滑点两 regime、min_cost 反解两 regime 与边界、整手（100 股 / SH688 / SH689 / 卖出零股）；`position.rs` 的 factor 调整（当日新买入不调整、停牌恢复补调、epsilon 比较）；`types.rs` 编解码往返。
2. **合成用例精确验收**（`tests/acceptance/` + `tests/acceptance_wap.rs` + `tests/data/synthetic/`）：3~5 只股票、约 10 个交易日、价格取整数/有限小数的手算用例，覆盖规范清单（涨跌停拦截、停牌、除权、退市估值沿用、min_cost 触发、整手不足一手、资金反解、两阶段核减、期初建仓（空仓首日买入 top_n）、deal_price 列无效（缺失/NaN/≤0）不可交易、**wap 模式方向价/方向量/缺失行/策略可见价**）。断言逐笔成交明细与逐日 account/value/cash **完全相等**，外加不变量（持仓 ≤ top_n、卖出截断、T+1、进度条开关不改变 daily/trades 输出）。每个用例同时附带手算预期值文件，作为正确性基准。
3. **端到端冒烟**（`tests/smoke_tmp_data.rs` + `tests/smoke_wap_data.rs`）：检测 `tmp_data/` 存在才运行（否则跳过），全区间跑通无 panic，校验输出文件的列名与日期格式；**不做数值对拍**（tmp_data 仅格式参考）。

---

## 9. 性能设计

- 主键：`Code(u32)` × `DayIdx(u32)`，哈希与比较开销最小。
- 存储：stock_bar 一次排序为 `(DayIdx, Code)`，`day_offsets` 支持 O(1) 日切片；信号按 `BTreeMap<NaiveDate, SignalDay>` 索引。
- 主循环内无 polars 逐行访问、无字符串操作、无日期解析；逐日堆分配通过 `Vec::with_capacity` 与复用缓冲控制。
- 批量计算（limit 预计算、校验、报表指标、导出反映射）全部走 polars 表达式/向量化，避免 for 循环（规范"非功能需求"）。
- 预期量级：5000 股 × 约 1500 交易日 ≈ 750 万行情行，主循环每行 O(1) 哈希查找；分钟级可达，无需并行化回测主循环（polars 内部已并行处理批量阶段）。
- 进度与计时零侵入：进度条禁用时 `Option<ProgressBar>` 为 None 无渲染开销；启用时每交易日一次 `inc(1)`，重绘由 indicatif 按时间节流；`Instant` 计时一次一读，开销可忽略。

---

## 10. 扩展点

| 扩展 | 预留 |
| --- | --- |
| 北交所 | 编码规则已兼容（4/8/9 数字段不重叠）；扩展 `format_instrument` 与整手规则表即可 |
| 新成交价类型（时段 VWAP 等） | `DealPrice` 枚举扩展 + `market.rs` 预计算分支；limit 的 `change` 取价随之切换，接口不变 |
| 分钟级数据 / 实盘 | 主循环的 `DayIdx` 抽象为 `BarIdx`；Exchange/Account 接口不依赖"日"语义，Strategy trait 不变 |
| 信号质量评估（IC/RankIC） | `Signal` 加载层已隔离 `ret`；新增独立模块复用 `load_signal` 的原始读取路径，不回灌回测 |
| 其他策略 | 实现 `Strategy` trait 即可：两阶段编排由 Backtest 通用提供，核减经 `revise_buy_orders` 默认钩子（`target_positions = None` 即退出）；排名/资金分配/股数换算复用 `strategy/common.rs`；需要历史信号时按 §4.6 的切窗方案扩展，不暴露完整 `Signal` |

---

## 11. 实现顺序建议

1. `types` + `error` + `data`（日历、stock_bar/benchmark 加载校验）——地基；
2. `signal` + `position` + `account`（含 factor 调整、估值）——领域核心；
3. `exchange/rules` 纯函数 + 单元测试——撮合规则先行固化；
4. `exchange/market` + `deal_order` 流水线；
5. `strategy/common` + `strategy/topk_dropout` + `backtest` 主循环（含核减钩子、进度条与耗时）；
6. `result` / `report`（指标、导出、plot）；
7. 合成验收用例 → tmp_data 冒烟 → 性能验证。
