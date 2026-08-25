//! 基础类型（架构 §4.1）：内部主键、字符串枚举、instrument 编解码、
//! strategy 与 exchange 共用的 `TradableInfo` / `StockTradable`。

use crate::error::{BtError, Result};

/// 交易日历索引：内部时间主键，0..n_days。
pub type DayIdx = u32;
/// 股票 int 编码：SH600000 -> 600000（规范"代码规范"）。
pub type Code = u32;

/// wap 时段价格类型（`deal_price = vwapN / twapN` 的种类，规范"Exchange 参数配置--deal_price"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WapKind {
    /// 时段成交量加权均价（`wap_N_*_vwap*` 列）
    Vwap,
    /// 时段时间加权均价（`wap_N_*_twap*` 列）
    Twap,
}

impl WapKind {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "vwap" => Ok(Self::Vwap),
            "twap" => Ok(Self::Twap),
            other => Err(BtError::InvalidParam(format!(
                "wap 价格类型仅支持 vwap/twap，收到: {other}"
            ))),
        }
    }

    /// 与 `parse` 互逆的规范名。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vwap => "vwap",
            Self::Twap => "twap",
        }
    }
}

/// wap 时段编号范围：wap_1 ..= wap_11（规范"数据文件格式--wap 数据"时段表）。
pub const WAP_WINDOW_MAX: u8 = 11;

/// 成交价格列选择（规范"Exchange 参数配置--deal_price"）。
///
/// `Wap` 变体为时段成交价（`vwapN` / `twapN`，N = 1..=11）：价格与成交量取自 wap 数据
/// （`wap_N_*_buy` / `wap_N_*_sell` 方向列），策略委托定价只可见 `pre_close`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DealPrice {
    Open,
    Close,
    Vwap,
    /// 时段 VWAP/TWAP：`kind` 选价格族，`window` 为时段号 1..=11。
    Wap { kind: WapKind, window: u8 },
}

impl DealPrice {
    /// 非法值返回 `Err`，不 panic。
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(Self::Open),
            "close" => Ok(Self::Close),
            "vwap" => Ok(Self::Vwap),
            other => {
                for (prefix, kind) in [("vwap", WapKind::Vwap), ("twap", WapKind::Twap)] {
                    if let Some(num) = other.strip_prefix(prefix) {
                        // 时段号须为无前导零的十进制（1..=11），与 Display 输出互逆
                        return match num.parse::<u8>() {
                            Ok(w) if !num.starts_with('0')
                                && (1..=WAP_WINDOW_MAX).contains(&w) =>
                            {
                                Ok(Self::Wap { kind, window: w })
                            }
                            _ => Err(BtError::InvalidParam(format!(
                                "deal_price 时段号须在 1..={WAP_WINDOW_MAX}（无前导零），收到: {other}"
                            ))),
                        };
                    }
                }
                Err(BtError::InvalidParam(format!(
                    "deal_price 仅支持 open/close/vwap/vwapN/twapN（N=1..={WAP_WINDOW_MAX}），收到: {other}"
                )))
            }
        }
    }

    /// 与 `parse` 互逆的规范名（配置回显 / 报错信息用，见 `Display`）。
    /// wap 模式对应 wap 数据方向列而非 stock_bar 单列，`column()` 返回 `None`。
    pub fn column(&self) -> Option<&'static str> {
        match self {
            Self::Open => Some("open"),
            Self::Close => Some("close"),
            Self::Vwap => Some("vwap"),
            Self::Wap { .. } => None,
        }
    }
}

impl std::fmt::Display for DealPrice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Close => write!(f, "close"),
            Self::Vwap => write!(f, "vwap"),
            Self::Wap { kind, window } => write!(f, "{}{}", kind.as_str(), window),
        }
    }
}

/// 超额收益口径（规范"指标定义--超额收益口径"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExcessMethod {
    /// 算术差 r − b
    Arithmetic,
    /// 几何差 (1+r)/(1+b) − 1
    Geometric,
}

impl ExcessMethod {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "arithmetic" => Ok(Self::Arithmetic),
            "geometric" => Ok(Self::Geometric),
            other => Err(BtError::InvalidParam(format!(
                "excess_method 仅支持 arithmetic/geometric，收到: {other}"
            ))),
        }
    }

    /// 与 `parse` 互逆的规范名（配置 / 报告接口用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Arithmetic => "arithmetic",
            Self::Geometric => "geometric",
        }
    }

    /// 组合单日超额收益。
    pub fn excess(&self, r: f64, b: f64) -> f64 {
        match self {
            Self::Arithmetic => r - b,
            Self::Geometric => (1.0 + r) / (1.0 + b) - 1.0,
        }
    }
}

/// 基准名称映射表（规范"数据文件格式--基准名称映射"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BenchmarkName {
    Hs300,
    Zz500,
    Cyb,
    Zz800,
    Zz1000,
    Zz2000,
    Sci,
    Kci,
    Cyi,
}

impl BenchmarkName {
    /// 名称不在映射表返回 `None`（调用方转 `Err`）。
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "hs300" => Self::Hs300,
            "zz500" => Self::Zz500,
            "cyb" => Self::Cyb,
            "zz800" => Self::Zz800,
            "zz1000" => Self::Zz1000,
            "zz2000" => Self::Zz2000,
            "sci" => Self::Sci,
            "kci" => Self::Kci,
            "cyi" => Self::Cyi,
            _ => return None,
        })
    }

    /// 与 `from_name` 互逆的规范名（配置 / 报告接口用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hs300 => "hs300",
            Self::Zz500 => "zz500",
            Self::Cyb => "cyb",
            Self::Zz800 => "zz800",
            Self::Zz1000 => "zz1000",
            Self::Zz2000 => "zz2000",
            Self::Sci => "sci",
            Self::Kci => "kci",
            Self::Cyi => "cyi",
        }
    }

    /// 数据文件中的指数代码。
    pub fn instrument(&self) -> &'static str {
        match self {
            Self::Hs300 => "SH000300",
            Self::Zz500 => "SZ399905",
            Self::Cyb => "SZ399006",
            Self::Zz800 => "SH000906",
            Self::Zz1000 => "SH000852",
            Self::Zz2000 => "CSI932000",
            Self::Sci => "CSIMJSC",
            Self::Kci => "CSIKC",
            Self::Cyi => "CSICY",
        }
    }
}

/// `instrument` -> `code`：`SH600000` -> 600000。仅接受 SH/SZ 前缀 + 6 位数字，
/// 且前缀与数字段首位必须匹配：沪市为 6xxxxx / 68xxxx，深市为 0xxxxx / 3xxxxx
/// （北交所 BJ 前缀及 4/8/9 开头数字段不在支持范围）。无法解析按数据校验报错。
pub fn parse_instrument(s: &str) -> Result<Code> {
    let bytes = s.as_bytes();
    if bytes.len() == 8 && bytes[2..].iter().all(u8::is_ascii_digit) {
        let code: Code = s[2..].parse().map_err(|_| {
            BtError::Validation(format!("instrument 数字段解析失败: {s}"))
        })?;
        let first_digit_ok = match &bytes[..2] {
            b"SH" => bytes[2] == b'6',
            b"SZ" => bytes[2] == b'0' || bytes[2] == b'3',
            _ => false,
        };
        if first_digit_ok {
            return Ok(code);
        }
    }
    Err(BtError::Validation(format!(
        "instrument 须为 SH/SZ 前缀 + 匹配的 6 位数字段（沪市 6 开头、深市 0/3 开头），收到: {s}"
    )))
}

/// `code` -> `instrument`：600000 -> `SH600000`。
/// 数字段首位 6 -> SH；0 / 3 -> SZ；其余（4/8/9，北交所段）当前报错。
pub fn format_instrument(code: Code) -> Result<String> {
    if code > 999_999 {
        return Err(BtError::Validation(format!("code 超出 6 位数字范围: {code}")));
    }
    let digits = format!("{code:06}");
    let prefix = match digits.as_bytes()[0] {
        b'6' => "SH",
        b'0' | b'3' => "SZ",
        other => {
            return Err(BtError::Validation(format!(
                "code {code}（数字段 {digits}）首位 {other} 属北交所段，暂不支持"
            )))
        }
    };
    Ok(format!("{prefix}{digits}"))
}

/// 是否科创板（SH688xxx / SH689xxx）：适用"200 股起、按 1 股递增"申报规则。
pub fn is_star_market(code: Code) -> bool {
    (688_000..=689_999).contains(&code)
}

/// 单只股票 T_exec 日可交易性视图（strategy 决策可见信息与 exchange 撮合共用口径，架构 D4）。
#[derive(Clone, Copy, Debug)]
pub struct StockTradable {
    /// 停牌：paused = 1 或 close 缺失
    pub suspended: bool,
    /// 涨停不可买入（按撮合买侧基价对 pre_close 判定）
    pub limit_buy: bool,
    /// 跌停不可卖出（按撮合卖侧基价对 pre_close 判定）
    pub limit_sell: bool,
    /// 当日买入侧成交量上限：volume × volume_threshold（wap 模式 = buy_volume × threshold）；
    /// threshold = None 时 +∞；无量 / 缺失为 0
    pub volume_cap: f64,
    /// 当日卖出侧成交量上限（普通模式与 `volume_cap` 同值；wap 模式 = sell_volume × threshold）
    pub sell_volume_cap: f64,
    /// 委托定价与金额换算用价格：普通模式 = T_exec 日 deal_price 列；wap 模式 = pre_close
    /// （决策时点合法已知，避免未来函数）；无效时为 NaN
    pub deal_price: f64,
    /// 是否 ST（盘前公开信息，供 forbid_st 使用）
    pub is_st: bool,
}

impl StockTradable {
    /// 是否可买入。
    pub fn buyable(&self) -> bool {
        !self.suspended
            && !self.limit_buy
            && self.volume_cap > 0.0
            && self.deal_price.is_finite()
            && self.deal_price > 0.0
    }

    /// 是否可卖出。
    pub fn sellable(&self) -> bool {
        !self.suspended
            && !self.limit_sell
            && self.sell_volume_cap > 0.0
            && self.deal_price.is_finite()
            && self.deal_price > 0.0
    }
}

/// T_exec 日全市场可交易性（按日 SoA 切片，code 升序，二分查找）。
///
/// 定义于基础层：strategy（决策可见）与 exchange（market.rs 构建）共用同一类型，
/// 两模块互不依赖且口径一致（架构 §4.6）。
#[derive(Clone, Copy, Debug)]
pub struct TradableInfo<'a> {
    pub(crate) codes: &'a [Code],
    pub(crate) suspended: &'a [bool],
    pub(crate) limit_buy: &'a [bool],
    pub(crate) limit_sell: &'a [bool],
    pub(crate) volume_cap: &'a [f64],
    pub(crate) sell_volume_cap: &'a [f64],
    pub(crate) deal_price: &'a [f64],
    pub(crate) is_st: &'a [bool],
}

impl<'a> TradableInfo<'a> {
    /// 查询单只股票；`None` 表示当日无行情。
    pub fn get(&self, code: Code) -> Option<StockTradable> {
        let i = self.codes.binary_search(&code).ok()?;
        Some(StockTradable {
            suspended: self.suspended[i],
            limit_buy: self.limit_buy[i],
            limit_sell: self.limit_sell[i],
            volume_cap: self.volume_cap[i],
            sell_volume_cap: self.sell_volume_cap[i],
            deal_price: self.deal_price[i],
            is_st: self.is_st[i],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_roundtrip() {
        assert_eq!(parse_instrument("SH600000").unwrap(), 600_000);
        assert_eq!(parse_instrument("SZ000006").unwrap(), 6);
        assert_eq!(parse_instrument("SZ300750").unwrap(), 300_750);
        assert_eq!(parse_instrument("SH688981").unwrap(), 688_981);
        assert_eq!(format_instrument(600_000).unwrap(), "SH600000");
        assert_eq!(format_instrument(6).unwrap(), "SZ000006");
        assert_eq!(format_instrument(300_750).unwrap(), "SZ300750");
        assert_eq!(format_instrument(688_981).unwrap(), "SH688981");
        for s in ["SH600000", "SZ000006", "SZ300750", "SH688981"] {
            assert_eq!(format_instrument(parse_instrument(s).unwrap()).unwrap(), s);
        }
    }

    #[test]
    fn instrument_invalid() {
        assert!(parse_instrument("BJ430047").is_err()); // 北交所不支持
        assert!(parse_instrument("SH60000").is_err()); // 长度不足
        assert!(parse_instrument("600000").is_err()); // 无前缀
        assert!(parse_instrument("SH60000A").is_err()); // 非数字
        assert!(parse_instrument("CSI932000").is_err()); // 指数代码不按股票编码
        assert!(parse_instrument("SZ600000").is_err()); // 前缀与数字段不匹配：沪市代码不能以 6 开头
        assert!(parse_instrument("SH000006").is_err()); // 深市代码不能以 SH 前缀
        assert!(parse_instrument("SH300750").is_err()); // 深市创业板代码不能以 SH 前缀
        assert!(format_instrument(430_047).is_err()); // 4 开头北交所段
        assert!(format_instrument(920_001).is_err()); // 9 开头
    }

    #[test]
    fn star_market() {
        assert!(is_star_market(688_981));
        assert!(is_star_market(689_009));
        assert!(!is_star_market(600_000));
        assert!(!is_star_market(300_750));
    }

    #[test]
    fn enum_parse() {
        assert_eq!(DealPrice::parse("vwap").unwrap(), DealPrice::Vwap);
        assert!(DealPrice::parse("avg").is_err());
        assert_eq!(ExcessMethod::parse("geometric").unwrap(), ExcessMethod::Geometric);
        assert!(ExcessMethod::parse("geo").is_err());
        assert_eq!(BenchmarkName::from_name("zz1000").unwrap().instrument(), "SH000852");
        assert!(BenchmarkName::from_name("CSI000400").is_none());
    }

    #[test]
    fn deal_price_wap_parse() {
        use WapKind::*;
        assert_eq!(
            DealPrice::parse("vwap11").unwrap(),
            DealPrice::Wap { kind: Vwap, window: 11 }
        );
        assert_eq!(
            DealPrice::parse("twap1").unwrap(),
            DealPrice::Wap { kind: Twap, window: 1 }
        );
        assert_eq!(
            DealPrice::parse("vwap5").unwrap(),
            DealPrice::Wap { kind: Vwap, window: 5 }
        );
        // 边界时段号合法
        assert_eq!(DealPrice::parse("twap11").unwrap().to_string(), "twap11");
        assert_eq!(DealPrice::parse("vwap1").unwrap().to_string(), "vwap1");
        // 非法：越界 / 裸 twap / 下划线 / 拼写 / 前导零
        for bad in [
            "vwap0", "vwap12", "twap99", "twap", "vwap_11", "wap11", "vwap11x", "VWAP11",
            "vwap01", "twap007",
        ] {
            assert!(DealPrice::parse(bad).is_err(), "应拒绝: {bad}");
        }
        // column：仅基础模式有 stock_bar 列
        assert_eq!(DealPrice::parse("open").unwrap().column(), Some("open"));
        assert_eq!(DealPrice::parse("vwap").unwrap().column(), Some("vwap"));
        assert_eq!(DealPrice::parse("twap11").unwrap().column(), None);
        // Display 与 parse 互逆
        for s in ["open", "close", "vwap", "vwap3", "twap11"] {
            assert_eq!(DealPrice::parse(s).unwrap().to_string(), s);
        }
    }
}
