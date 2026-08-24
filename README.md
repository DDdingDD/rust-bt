# rust-bt

Rust 股票回测系统（A 股日频）。设计依据：

- `doc/specification.md` -- 设计规范（行为口径的唯一权威）
- `doc/architecture.md` -- 系统架构（模块划分、类型设计、关键决策）
- `doc/api.md` -- 嵌入 API 文档（嵌入其他 Rust 代码的接口与调用示例）

## 构建与运行

```bash
cargo build --release
# 端到端示例（数据路径指向 tmp_data/，可在 examples/run_backtest.rs 中修改）
cargo run --release --example run_backtest
# 嵌入 API 示例（内存信号 + 自定义策略，见 doc/api.md）
cargo run --release --example embed_api
```

输出统一写入 `output/`（已 gitignore）：`hist_position.csv`、`trades.csv`、`report_data.csv`、`report_plot.html`。

## 测试

```bash
cargo test --lib                                    # 单元测试（rules/types/position/calendar）
cargo test --test '*'                               # 合成用例精确验收（手算对拍）
cargo test --release --test smoke_tmp_data          # tmp_data 端到端冒烟（数据缺失自动跳过）
```

## 用法

- **嵌入其他 Rust 代码**（推荐）：一次 `run` 调用完成装配 + 回测 + 报告，
  参数类型化、结果内存消费。接口文档与调用示例见 `doc/api.md`，
  可运行示例 `examples/embed_api.rs`。
- **组件层细粒度编排**：见 `examples/run_backtest.rs`，与规范"使用方法"一致：
  加载信号与行情 -> 配置 Exchange（成交价 / 费率 / 滑点 / 成交量与涨跌停约束）->
  `TopkDropoutStrategy`（或自定义 `Strategy` trait 实现）-> `Backtest::run` ->
  导出持仓 / 成交 / 报表。

## 作为依赖引入

```toml
# 方式一：私有 git + tag（推荐；当前已有 v0.1.0）
rust-bt = { git = "ssh://git@codeup.aliyun.com:641c2b0a467b1259b4792ed4/rust-bt.git", tag = "v0.1.0" }

# 方式二：GitHub 镜像（同步推送，任选其一）
rust-bt = { git = "ssh://git@github.com:DDdingDD/rust-bt.git", tag = "v0.1.0" }

# 方式三：始终跟踪 main（开发集成，不推荐生产使用）
rust-bt = { git = "ssh://git@codeup.aliyun.com:641c2b0a467b1259b4792ed4/rust-bt.git", branch = "main" }
```

- 当前版本：`v0.1.0`（嵌入 API / DataFrame 信号 / 简报）
- 升级：更新 `tag` 后执行 `cargo update`；首次拉取后 cargo 会缓存，必要时删除
  `~/.cargo/git/checkouts` 下对应目录强制刷新
- 远端说明：`origin` 为阿里云 CodeUp（主仓库），`github` 为 GitHub 镜像；
  两者 `main` 与 tag 均保持同步

## 发布流程（维护者）

```bash
# 1. 确保 main 已推送并验证通过
git push origin main
cargo test --lib && cargo test --test '*'

# 2. 打 annotated tag（语义化版本）
VERSION=v0.1.1
git tag -a $VERSION -m "$VERSION: xxx"

# 3. 推送 tag 到双远端
git push origin main $VERSION
git push github main $VERSION
```

注意：Rust 库以源码分发，第三方 `cargo build` 时自动拉取并编译；
`cargo package --list` 可查看会被分发的文件（已排除 `tmp_data/`、`output/`、`target/`）。
