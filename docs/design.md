# VTX 设计方案：用 egui + egui_dock 复刻 wezterm 的基础 VT 能力

本文是 VTX 的实现蓝图，按"可独立实现、可独立测试"的粒度切成模块。每个模块都给出：

- 职责与边界
- 拟定的公开 API（Rust 草案）
- 关键实现要点（含容易踩的坑）
- **参考路径**：wezterm（`refer/wezterm/...`，含已验证的行号），关键处另附 Konsole（`refer/konsole/...`）对照
- 验收标准

范围：**只做基础 VT 能力**。配色方案/主题系统、SSH/mux server、图形协议（sixel/iterm2/kitty）、鼠标上报、超链接、连字、bidi 一律推迟。

**§8 是横向对比**：把 wezterm 与 Konsole 放在同一张桌子上，先给规模实测（§8.2）与逐模块三方对照（§8.3），再深入分析两家给出**相反答案**的八处分歧（§8.4），最后是"VTX 每个模块抄谁、哪些都不抄"的结论（§8.5）。**动手前建议先读 §8.5。**

---

## 0. 术语对照

| wezterm | VTX | 说明 |
| --- | --- | --- |
| `pty::PtySize` / `MasterPty` | `vtx_core::pty::PtySize` / 直接用 `portable_pty::MasterPty` | wezterm 是 `portable-pty` 的**上游**，vtx 直接依赖同款 crate |
| `vtparse::VTParser` / `VTActor` | 同 | 同一个 crate（0.7），vtx 直接复用 |
| `wezterm-escape-parser` 的 `Action` | `vtx_core::escape::Action` | vtx 不依赖该 crate，**需要手写精简版** |
| `term::TerminalState` | `vtx_core::terminal::TerminalState` | 模式位、光标、边距、alt screen |
| `wezterm-surface` 的 `Surface` / `Line` | `vtx_core::screen::Screen` / `vtx_core::line::Line` | vtx 已有 `Line`，需演化 |
| `mux::Pane` / `LocalPane` | `vtx_core::pane::Pane` | PTY + 终端模型 + 读线程的聚合 |
| `termwiz::input::KeyEvent::encode` | `vtx_core::input::encode_key` | 键事件 → 字节序列 |
| `wezterm-gui` `termwindow` / `glyphcache` | `vtx-ui::terminal_view` | 渲染 |
| `mux::Window` / `Tab` | `vtx-ui::terminal_tab`（egui_dock 承载） | 多标签/分屏 |

---

## 1. 现状盘点

已有（可继续用）：

| 文件 | 内容 |
| --- | --- |
| [crates/vtx-core/src/cell.rs](crates/vtx-core/src/cell.rs) | `Color`、`Intensity`、`Underline`、`Blink`、`CellAttrs`（u32 位域）、`CellText`（`Char`/`Grapheme`）、`Cell` |
| [crates/vtx-core/src/line.rs](crates/vtx-core/src/line.rs) | `Line { cells, seqno }`、`write_cell`、`last_cell_was_wrapped`、`resize`、`is_whitespace` |
| [crates/vtx-core/src/session.rs](crates/vtx-core/src/session.rs) | **待替换的 mock**：`write_str` 手喂文本、立即换行、无模式、无 scrollback |
| [crates/vtx-ui/src/theme.rs](crates/vtx-ui/src/theme.rs) | `Theme`、`indexed(u8)`（16 色 + 6×6×6 立方 + 灰阶） |
| [crates/vtx-ui/src/terminal_view.rs](crates/vtx-ui/src/terminal_view.rs) | 行 galley 缓存 + run 合并 + 光标块 |
| [crates/vtx-ui/src/terminal_tab.rs](crates/vtx-ui/src/terminal_tab.rs) | `DockState<Pane>`、`TabViewer`、`on_add` 延迟处理 |
| [crates/vtx-app/src/main.rs](crates/vtx-app/src/main.rs) | eframe 入口 |

已声明但**尚未使用**的依赖（`crates/vtx-core/Cargo.toml`）：`portable-pty`、`tokio`、`vtparse`、`unicode-width`。

关键结论：

1. `session.rs` 是要被 `screen.rs` + `terminal.rs` 取代的 mock。它的两处语义偏差必须修掉：
   - `put_char` **立即换行**（真实终端是**延迟换行**，见 M3/M4）；
   - `scroll_up` 后**重刷所有行的 seqno**（注释里也已自认是权宜之计），要靠**稳定行号**解决（见 M4）。
2. `terminal_view.rs` 的渲染路径要做一次决策（galley vs 逐字形），见 M7。
3. `terminal_tab.rs` **没有实现 `on_close`**，所以目前关标签不会杀子进程 —— M8 必须补。

---

## 2. 目标架构

### 2.1 分层

```
                 ┌──────────────────────────────────────────────┐
  egui/eframe    │ vtx-app   : eframe::App                      │
  (UI 线程)      ├──────────────────────────────────────────────┤
                 │ vtx-ui    : TerminalTab / TerminalView       │
                 │             渲染 + 输入事件 + 尺寸联动        │
                 ├──────────────────────────────────────────────┤
                 │ vtx-core  : Pane                             │
                 │   ├─ pty      : 生成/读写/resize/kill         │
                 │   ├─ parser   : vtparse 驱动 + 语义 Action 层  │
                 │   ├─ screen   : 行缓冲 / scrollback / 边距     │
                 │   ├─ terminal : 模式位 / 光标 / alt screen     │
                 │   └─ input    : 键事件 → 字节序列              │
                 └──────────────────────────────────────────────┘
                              ↓ read / write / resize
                        OS PTY（ConPTY 或 openpty）
```

### 2.2 线程模型

每打开一个 pane，起一个 **reader 线程**（阻塞 read，1 MiB 缓冲），在同一个线程里直接解析并写入共享终端模型。写方向是 `Box<dyn Write + Send>`，由 UI 线程加锁写。

```
UI 线程 (eframe/egui)
  ├─ 每帧: TerminalView::show()
  │    ├─ 加锁读取 Arc<Mutex<TerminalState>> 快照 → 绘制
  │    ├─ 收集 egui 事件 → encode_key → 加锁写 pty writer
  │    └─ 尺寸变化 → TerminalState::resize() + MasterPty::resize()
  └─ ctx.request_repaint()  ◄──────────────┐
                                          │
reader 线程 (每 pane 一个)                  │
  read(1MiB) → Parser::advance → TerminalState::perform_actions
             → 解锁 → ctx.request_repaint() ┘
```

**跨线程唤醒**：reader 线程持有 `egui::Context` 的一个 clone。已核实 egui 0.36 的 `Context` 是 `Arc<RwLock<ContextImpl>>`（`egui-0.36.1/src/context.rs:722`），因此 `Clone + Send + Sync`，`Context::request_repaint`（同文件 `:1818`）可在任意线程调用。这**替代** wezterm 的 `MuxNotification` → `window.invalidate()` 唤醒链。

**共享状态**：`Arc<Mutex<TerminalState>>`，对应 wezterm 的 `Pane::terminal() -> Arc<FairMutex<TerminalState>>`。锁的粒度是"一次 read chunk 的处理"，即 wezterm 的"一次 `perform_actions` 事务"。

### 2.3 一帧数据流

```
read chunk
  → vtparse 状态机 (字节级，仅识别序列边界)
  → VTActor 回调 (print/execute/csi_dispatch/esc_dispatch/osc_dispatch)
  → 语义层: 回调转成 Vec<Action>  (vtx 手写，wezterm 是 wezterm-escape-parser)
  → TerminalState::perform_actions(actions)  ← 一个事务，seqno 只 +1
       ├─ Screen 改行 → 行 seqno 更新
       └─ 记录脏行集合
  → ctx.request_repaint()
  → TerminalView::show()
       └─ 逐行比较 (stable_row_index, seqno) → 只重 shape 脏行
```

---

## 3. 模块清单与实施顺序

| # | 模块 | 新增/改造文件 | 依赖 | wezterm 主要参考 |
| --- | --- | --- | --- | --- |
| M1 | PTY 层 | `vtx-core/src/pty.rs`（新） | — | `pty/src/lib.rs`、`pty/src/unix.rs`、`pty/src/win/*`、`mux/src/localpane.rs` |
| M2 | 解析器接线 | `vtx-core/src/parser.rs`（新） | — | `wezterm-escape-parser/src/parser/mod.rs` |
| M3 | 语义层（Action） | `vtx-core/src/escape/{mod,action,csi,esc,osc}.rs`（新） | M2 | `wezterm-escape-parser/src/{csi,esc,osc}.rs`、`term/src/terminalstate/performer.rs` |
| M4 | 屏幕模型 | `vtx-core/src/screen.rs`、`terminal.rs`（新；`session.rs` 退役） | M3 | `wezterm-surface/src/lib.rs`、`.../line/line.rs`、`term/src/screen.rs`、`term/src/terminalstate/mod.rs` |
| M5 | 输入编码 | `vtx-core/src/input.rs`（新） | M1 | `termwiz/src/input.rs`、`wezterm-input-types/src/lib.rs`、`wezterm-gui/src/termwindow/keyevent.rs` |
| M6 | 尺寸联动 | 贯穿 M1/M4/M7 | M1, M4 | `wezterm-gui/src/termwindow/resize.rs`、`mux/src/pane.rs` |
| M7 | 渲染改造 | `vtx-ui/src/terminal_view.rs`（改造） | M4 | `wezterm-gui/src/termwindow/render/{screen_line,mod,paint}.rs`、`glyphcache.rs` |
| M8 | dock 集成 + 生命周期 | `vtx-ui/src/terminal_tab.rs`（改造） | M1, M5, M7 | `mux/src/{pane,localpane,window,tab}.rs`、`wezterm-gui/src/termwindow/mod.rs` |

**依赖图与建议顺序**：

```
M1 ─┬─→ M5 ─┐
    │        │
M2 → M3 → M4 ┴→ M6 → M7 → M8
```

M1、M2+M3 可以并行先做（M2/M3 是纯函数，无需 PTY 就能测）。**M4 是整个项目的重心**，其余模块围绕它。M6 本质上不是一个独立模块，而是 M1+M4+M7 之间的契约，单独列出只是为了明确责任划分。

---

## 4. 模块详设

### M1 PTY 层

**职责**：生成 PTY 与子进程；提供 reader/writer/killer/resize 四个句柄；把子进程退出变成一条可等待的通知。**不含**任何 VT 语义。

**API 草案**

```rust
// crates/vtx-core/src/pty.rs
pub use portable_pty::PtySize;

/// 启动一个 PTY 及其子进程后拿到的全部句柄。
pub struct PtyHandles {
    /// 用于 resize 的 master 端。
    master: Box<dyn MasterPty + Send>,
    /// 阻塞读取子进程输出；交给 reader 线程独占。
    reader: Box<dyn Read + Send>,
    /// 写入子进程输入；UI 线程加锁使用。
    writer: Box<dyn Write + Send>,
    /// 可跨线程克隆的杀进程句柄。
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// 子进程；`wait()` 应在独立线程调用。
    child: Box<dyn Child + Send + Sync>,
}

pub struct SpawnConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub size: PtySize,
}

pub fn spawn(config: SpawnConfig) -> io::Result<PtyHandles>;
impl PtyHandles {
    pub fn resize(&self, size: PtySize) -> io::Result<()>;
    pub fn kill(&self) -> io::Result<()>;
    /// 在独立线程里调用；返回退出状态。
    pub fn wait(&mut self) -> io::Result<ExitStatus>;
    pub fn take_reader(&mut self) -> Box<dyn Read + Send>;
    pub fn take_writer(&mut self) -> Box<dyn Write + Send>;
    pub fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync>;
}
```

**实现要点**

1. `portable_pty::native_pty_system().openpty(size)` → `(master, slave)`；`slave.spawn_command(cmd)` 得 `Child`；`master.try_clone_reader()` 与 `master.take_writer()`。
2. **启动后立刻 drop `slave`**。wezterm 明确这么做：slave 端只要还开着，子进程退出后 master 就读不到 EOF，reader 线程会挂着不退出。
3. reader 循环：`read(&mut buf[..])`；`Ok(0)` = EOF = 子进程结束；`Err(e) if e.kind() == Interrupted` 继续；其他错误记录后结束。缓冲区取 1 MiB（wezterm 的值）。
4. **`wait()` 必须单独起线程**（阻塞调用）。wezterm 用 `smol::channel::bounded(1)` 把退出状态送回；vtx 用 `std::sync::mpsc`，收到后 `ctx.request_repaint()`。
5. `killer` 要提前 `clone_killer()` 出来。`Child` 自身被 drop 时也会杀进程，但我们需要"关标签时显式杀"的确定性，不能依赖 drop 顺序。
6. resize 走 `master.resize(PtySize { cols, rows, pixel_width, pixel_height })`。unix 下是 `TIOCSWINSZ`，Windows 下是 `ResizePseudoConsole` —— `portable-pty` 已封装，vtx 不需要写平台代码。
7. Windows 上 `program` 需要能被 `CreateProcess` 解析；默认 shell 建议 `COMSPEC`（`cmd.exe`）或直接 `powershell.exe`；unix 用 `$SHELL`，回退 `/bin/sh`。
8. 依赖清理：**移除 `tokio`**。`portable-pty` 是阻塞 API，wezterm 用的是 `std::thread` + `smol` 通道；vtx 用 `std::thread` + `std::sync::mpsc` 即可，引入 tokio 只会带来一个用不上的运行时。

**wezterm 参考**

- `refer/wezterm/pty/src/lib.rs`：`PtySize`(:~40)、`MasterPty`、`Child`、`ChildKiller`(:150，含 `clone_killer`)、`SlavePty`、`PtySystem`、`native_pty_system` —— vtx 的 API 草案就是这套 trait 面的裁剪版。
- `refer/wezterm/pty/src/cmdbuilder.rs`：`CommandBuilder`（vtx 用 `portable_pty::CommandBuilder` 即可，无需自写）。
- `refer/wezterm/pty/src/unix.rs`：`openpty` + fork/exec，`setsid`、`TIOCSCTTY`（控制终端）、`TIOCSWINSZ`（resize）、SIGHUP→SIGKILL 的杀进程策略。**看它是为了理解语义，不需要抄代码。**
- `refer/wezterm/pty/src/win/conpty.rs`、`win/pseudocon.rs`、`win/procthreadattr.rs`：`CreatePseudoConsole` + `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` + `ResizePseudoConsole` + `TerminateProcess`。
- `refer/wezterm/mux/src/localpane.rs`：**最有价值的参考** —— `spawn` 之后如何组织 reader 线程、如何用 `ChildKiller`、如何把 `wait()` 的结果变成 `Pane` 的死活状态。

**验收**：单测在无 tty 环境要能跳过；集成测试 spawn 一个打印固定字符串的短命令（win: `cmd /c echo hi`，unix: `sh -c 'echo hi'`）并断言读到 `hi`；再测一次 `resize` 不报错、`kill` 后 `wait()` 能返回。

---

### M2 解析器接线

**职责**：把字节流喂给 `vtparse` 状态机，并把回调转交给语义层。**只做胶水**。

**API 草案**

```rust
// crates/vtx-core/src/parser.rs
pub struct Parser { inner: vtparse::VTParser }

impl Parser {
    pub fn new() -> Self;
    /// 把一段字节喂给状态机；回调进入 performer。
    pub fn advance(&mut self, bytes: &[u8], performer: &mut dyn vtparse::VTActor);
}
```

**已核实的关键事实（vtparse 0.7.0）**

- `pub struct VTParser`（`vtparse-0.7.0/src/lib.rs:361`），`VTParser::new()`，`parse(&mut self, bytes: &[u8], actor: &mut dyn VTActor)`（`:741`），另有 `parse_byte(&mut self, byte: u8, actor: &mut dyn VTActor)`（`:713`）。
- `pub enum CsiParam { Integer(i64), P(u8) }`（`:401`），带 `as_integer() -> Option<i64>`（`:413`）。**私有前缀字符就是 `P(b'?')` 这类值** —— SGR 的 `38;5;n` 与 `\x1b[?25h` 的识别都靠这个。
- `VTActor` 的方法签名（`vtparse-0.7.0/src/lib.rs:90` 起）：
  ```rust
  fn print(&mut self, c: char);
  fn execute_c0_or_c1(&mut self, byte: u8);
  fn dcs_hook(&mut self, mode: u8, params: &[i64], intermediates: &[u8], ignored_excess_intermediates: bool);
  fn dcs_put(&mut self, byte: u8);
  fn dcs_unhook(&mut self);
  fn esc_dispatch(&mut self, params: &[i64], intermediates: &[u8], ignored_excess_intermediates: bool, byte: u8);
  fn csi_dispatch(&mut self, params: &[CsiParam], parameters_truncated: bool, byte: u8);
  fn osc_dispatch(&mut self, params: &[&[u8]]);
  fn apc_dispatch(&mut self, data: Vec<u8>);
  ```
  全部有默认空实现，vtx 只覆写需要的。
- crate 文档**明确声明它不赋予转义序列语义**（"It does not ascribe semantic meaning to those escape sequences"）—— 这就是 M3 必须手写的原因。
- `parameters_truncated` 表示参数个数超过内部 `MAX_PARAMS` 被截断，遇到时至少记一条日志。

**实现要点**

1. performer 需要能被强转成 `&mut dyn VTActor`；把 `TerminalState` 直接实现 `VTActor` 最省事，但**推荐拆成两步**：`TerminalState` 实现 `VTActor`，其 `csi_dispatch` 等回调**只负责把序列翻译成 `Action` 塞进 `Vec<Action>`**，真正的 apply 由 `perform_actions(&mut self, actions: Vec<Action>)` 完成。理由：
   - 一个 read chunk 的多次回调合并成**一个事务**（seqno 只 +1），脏行集合一次算完；
   - 语义层可以纯函数化、单测无需 PTY；
   - 与 wezterm 的 `parse_buffered_data` → `perform_actions` 结构一致，将来要加 coalescing 或跨线程解析都不用改上层。
2. **UTF-8 与 C1 的坑**：子进程输出的 8-bit C1（`0x84` IND 等）在 UTF-8 流里是 `0xC2 0x84` 两字节。vtparse 的 `Utf8Sequence` 状态会把它识别回来并调 `execute_c0_or_c1(0x84)`。实现 M3 时要为 C1 写测试确认这一点。
3. 建议在 `Pane` 层就固定"一次 read = 一次 `advance` = 一次 `perform_actions`"，不要在中间再做额外缓冲。

**wezterm 参考**

- `refer/wezterm/vtparse/src/lib.rs`（与已发布的 0.7 同源，可直接对照状态表）：`State` 枚举、`lookup` 状态表、`Utf8Parser`。
- `refer/wezterm/wezterm-escape-parser/src/parser/mod.rs`：看 `Parser::parse` 如何驱动 vtparse、如何处理 `parameters_truncated` 与中间字节。
- `refer/wezterm/mux/src/lib.rs`：搜 `parse_buffered_data` —— 看"read 缓冲 → 解析 → 收集动作 → perform"这条链怎么切分事务。

**验收**：`Parser::advance(b"a\x1b[31mb", collector)` 能得到 `Print('a')`、`CsiDispatch{params:[31], byte:'m'}`、`Print('b')` 三条动作（可用 `vtparse::CollectingVTActor` 做对照测试）。

---

### M3 语义层（Action）

**职责**：把 vtparse 的**字节级回调**翻译成**结构化指令**，并实现 SGR 这类需要多参数解析的复杂序列。这是"VT 语义"的所在，也是 vtx 必须原创的部分（因为不依赖 `wezterm-escape-parser`）。

**API 草案**

```rust
// crates/vtx-core/src/escape/action.rs
pub enum Action {
    /// 打印一个字符（尚未成簇，见下方"字符簇"）。
    Print(char),
    /// C0/C1 控制字符。
    Control(C0),
    /// 光标移动。
    CursorMove(CursorMove),
    /// 屏幕/行编辑。
    Erase(EraseInDisplay),
    EraseLine(EraseInLine),
    InsertLines(usize), DeleteLines(usize),
    InsertChars(usize), DeleteChars(usize), EraseChars(usize),
    /// 滚动。
    ScrollUp(usize), ScrollDown(usize),
    /// SGR。
    SetAttributes(AttributeChange),
    /// 模式开关（含 DEC 私有模式）。
    SetMode { mode: Mode, enable: bool },
    /// 边距。
    SetScrollingRegion { top: usize, bottom: usize },
    SetLeftRightMargin { left: usize, right: usize },
    /// 设备查询：需要"应答"，由 TerminalState 产生回写字节。
    DeviceStatusReport(DsrKind),
    DeviceAttributes,
    /// 保存/恢复光标、tab、reset。
    SaveCursor, RestoreCursor, SetTabStop, ClearTabStops,
    SetCursorStyle(CursorShape),
    SetTitle(String),
    // DCS/APC/OSC 中被推迟的部分在此留 enum 位，但暂不实现行为。
}
```

**第一版必须覆盖的集合**（其余一律"识别但忽略"，并 debug 日志）：

- **C0**：`BS`(0x08) `HT`(0x09) `LF`(0x0A) `VT`(0x0B) `FF`(0x0C) `CR`(0x0D) `SO`(0x0E) `SI`(0x0F) `BEL`(0x07) `ESC`(0x1B)。
- **C1**：`IND`(0x84) `NEL`(0x85) `HTS`(0x88) `RI`(0x8D) `DECID`(0x9A)。
- **CSI**：`A/B/C/D`(CUU/CUD/CUF/CUB)、`E/F`(CNL/CPL)、`G`(CHA)、`H`/`f`(CUP/HVP)、`J`(ED)、`K`(EL)、`L`/`M`(IL/DL)、`P`/`@`(DCH/ICH)、`X`(ECH)、`S`/`T`(SU/SD)、`c`(DA)、`n`(DSR)、`g`(TBC)、`m`(SGR)、`r`(DECSTBM)、`s`(DECSLRM，需 69 模式)、`h`/`l`(SM/RM) 与私有形式 `?h`/`?l`(DECSET/DECRST)、`q`(DECSCUSR)。
- **ESC**：`7`/`8`(DECSC/DECRC)、`=`/`>`(DECKPAM/DECKPNM)、`c`(RIS)、`M`(RI)、`D`(IND)、`E`(NEL)、`H`(HTS)。
- **OSC**：`0`/`1`/`2`(title)。`4`(调色板)、`7`(cwd)、`8`(超链接)、`10`/`11`(fg/bg)、`52`(剪贴板)、`133`/`633`(shell integration) 推迟。
- **DECSET/DECRST 模式**（第一版）：`1`(DECCKM 应用光标键)、`5`(DECSCNM 反显)、`6`(DECOM 原点相对)、`7`(DECAWM 自动换行)、`25`(DECTCEM 光标可见)、`47`/`1047`/`1049`(alt screen)、`1000`/`1002`/`1003`/`1006`/`1015`(鼠标)、`2004`(bracketed paste)、`69`(DECSLRM 左右边距)。
- **SGR 全表**（vtx 的 `CellAttrs` 位域已经能表达，见 [cell.rs](crates/vtx-core/src/cell.rs)）：`0,1,2,3,4,5,6,7,8,9,21,22,23,24,25,27,28,29,30-37,38;5;n,38;2;r;g;b,39,40-47,48...,49,90-97,100-107`。

**实现要点**

1. **字符簇（grapheme）**：`VTActor::print` 是**逐 `char`** 回调，而 `CellText::Grapheme(Arc<str>)` 需要整簇。做法：performer 里维护一个"待冲刷的字符缓冲"，在遇到非 `print` 回调或 chunk 结束时，用 `unicode-segmentation::UnicodeSegmentation::graphemes` 切簇再逐簇写单元格。宽度用 `unicode-width`（已是依赖；`CellText::width()` 已按 2 封顶）。
2. **延迟换行（`wrap_next`）**：这是与当前 mock 最重要的差别。写到最后一列时**不换行**，只置 `wrap_pending`；下一个可打印字符到来时才真正换行，并给上一行最后单元格打 `wrapped` 标记（`Line::set_last_cell_was_wrapped`）。光标停在最后一列期间，`CR`/`LF` 会取消 `wrap_pending`。**参考 wezterm 的 `wrap_next` 实现，这是正确性的关键点之一。**
3. **SGR 多参数**：`38`/`48` 需要向后看 `5;n` 或 `2;r;g;b`，`CsiParam` 是切片，直接按索引读即可。注意 `CsiParam::P(b'?')` 之类的私有标记要单独判断。
4. **需要应答的序列**（DSR/DA）会在 `TerminalState` 侧产生回写字节（`\x1b[0n`、`\x1b[?1;2c` 之类），走 writer 写回 pty。第一版至少支持 `DSR 5`(OK) 与 `DSR 6`(CPR，返回 `\x1b[{row};{col}R`)，因为不少 shell/程序依赖 CPR。
5. 未识别的序列不要静默吞掉，打成 `debug!` 日志，方便和 wezterm 对拍。

**wezterm 参考**

- `refer/wezterm/wezterm-escape-parser/src/csi.rs`：`CsiHandler`/`CsiParser`、SGR 解析、DECSET/DECRST 模式表 —— **M3 的主要参考**。
- `refer/wezterm/wezterm-escape-parser/src/esc.rs`、`osc.rs`、`apc.rs`、`lib.rs`（`Action` 枚举全貌，可按需要裁剪）。
- `refer/wezterm/term/src/terminalstate/performer.rs`：`impl Perform for TerminalState`，每个 `Action` 落到哪个方法。
- `refer/wezterm/wezterm-surface/src/cellcluster.rs`：`CellCluster` 如何把单元格聚成可绘制的 run —— 对 M3 的字符簇写入与 M7 的 run 切分都直接有用。

**验收**：纯单测。把 wezterm 的 `refer/wezterm/term/src/test/csi.rs` 用例翻译过来是最省力也最权威的做法（见 §5）。

---

### M4 屏幕模型

**职责**：终端的状态机与缓冲。拆成两半：`Screen`（行缓冲、scrollback、边距）与 `TerminalState`（模式、光标、pen、alt screen 切换、动作的 apply）。

**API 草案**

```rust
// crates/vtx-core/src/screen.rs
pub struct Screen {
    lines: Vec<Line>,             // 可见区，长度 = rows
    scrollback: VecDeque<Line>,   // 顶部滚出的历史
    scrollback_limit: usize,      // 默认 3500
    /// 可见区第一行的稳定行号；滚动时递增，行内容不重编号。
    stable_row_offset: u64,
    cols: usize,
    rows: usize,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self;
    pub fn resize(&mut self, cols: usize, rows: usize, reflow: bool);
    pub fn scroll_up(&mut self, n: usize);          // 进 scrollback
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize, n: usize);
    pub fn line(&self, row: usize) -> &Line;
    pub fn line_mut(&mut self, row: usize) -> &mut Line;
    /// 行的稳定标识，供渲染缓存做 key。
    pub fn stable_row_index(&self, row: usize) -> u64;
    pub fn clear_scrollback(&mut self);
}

// crates/vtx-core/src/terminal.rs
pub struct TerminalState {
    primary: Screen,
    alt: Screen,
    alt_active: bool,

    cols: usize,
    rows: usize,

    cursor_x: usize,
    cursor_y: usize,
    wrap_pending: bool,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    saved_cursor: Option<SavedCursor>,

    pen: CellAttrs,
    modes: Modes,

    scroll_top: usize,      // DECSTBM
    scroll_bottom: usize,
    margin_left: usize,     // DECSLRM
    margin_right: usize,

    tab_stops: Vec<bool>,
    title: String,

    seqno: SequenceNo,      // 当前事务号
    dirty: Vec<Range<usize>>, // 或 BitSet，供渲染增量刷新
    /// 需要回写给子进程的字节（DSR/DA 应答）。
    responses: Vec<u8>,
    /// 子进程已退出及其状态（由 M1 的 wait 线程置入）。
    exited: Option<ExitStatus>,
}
```

**实现要点**

1. **稳定行号**：给每行一个单调递增的 `stable_row_index`，滚动时**不重编号**（`stable_row_offset += n`），只让 `Screen::stable_row_index(row)` 随偏移变化。这样渲染缓存的 key 用 `(stable_row_index, seqno)` 就能在滚动后自然命中未变动的行 —— 直接干掉 [session.rs](crates/vtx-core/src/session.rs) 里"滚动后重刷所有 seqno"的 hack。wezterm 的对应概念是 `SequenceNo`（`refer/wezterm/wezterm-surface/src/lib.rs:90`）与 `stable_row_index`。
2. **模式位**：`Modes` 用位域手写（与 [cell.rs](crates/vtx-core/src/cell.rs) 的 `CellAttrs` 风格保持一致），不用 `bitflags`。字段见 M3 的模式清单。
3. **alt screen**：两份 `Screen` + `alt_active`。`1049` 语义是"切到 alt、清空它、保存光标"；退出时恢复主屏与光标。`47`/`1047` 不清屏。参考 wezterm 的 `ScreenOrAlt`。
4. **边距**：`DECSTBM` 设上下边距，`DECSLRM` 设左右边距（需 69 模式）。**所有滚动/插入/删除操作只在边距内生效**，`CUP` 在 DECOM 模式下以 `scroll_top` 为原点。这是最容易写错的地方，务必对照 wezterm。
5. **scrollback**：`scroll_up` 时把顶部行 `push_back` 进 `VecDeque`，超限 `pop_front`。`ED 3` 清 scrollback。注意：只有"因滚动而离开可见区"的行才进 scrollback，`ED 2` 清屏不进。
6. **SGR 的 pen 更新**：`22` 要同时清 bold 与 half，`24` 只清下划线，`0` 全清 —— 逐个对照 wezterm 的 `CellAttributes` 实现。
7. **resize**：第一版 **不做 reflow**（wezterm 有完整实现，见 `refer/wezterm/term/src/terminalstate/mod.rs` 的 reflow 路径）。策略：
   - 行数减少 → 顶部行移入 scrollback（或按边距裁剪）；行数增加 → 底部补空行；
   - 列数变化 → 调 `Line::resize(new_cols, will_wrap)`（[line.rs](crates/vtx-core/src/line.rs) 已有），内容超出直接截断（**已知缺陷，在代码注释里写明 reflow 待做**）；
   - 光标钳制到新网格内。
8. **动作的 apply**：`perform_actions(actions)` 是唯一入口；事务开始时记录起始 seqno，结束时 `seqno += 1` 并重算脏行集合。行修改必须走 `Line::write_cell(pos, cell, seqno)` 之类会更新行 seqno 的接口，否则渲染不知道自己脏了。
9. `title` 存在这里，M8 负责推给 egui。
10. `responses` 由 M8 在每帧取出写给 pty。

**wezterm 参考**

- `refer/wezterm/wezterm-surface/src/lib.rs`：`Surface`/`Change`/`SequenceNo`(:90) —— `SequenceNo` 与"变更日志"的设计。
- `refer/wezterm/wezterm-surface/src/line/line.rs`：`Line::resize`、`set_last_cell_was_wrapped`、`is_whitespace`、`LineBits`。
- `refer/wezterm/wezterm-surface/src/line/clusterline.rs` 与 `.../vecstorage.rs`：两种行存储（稀疏/稠密）。**vtx 只做稠密**（已有 `Vec<Cell>`），在文档里说明这是刻意的取舍（wezterm 的稀疏存储是为内存优化，vtx 暂不需要）。
- `refer/wezterm/term/src/screen.rs`：`Screen` 的 `perform_actions`、滚动/选择、`stable_row_index`、`wrap_next` 的处理 —— **M4 的主要参考**。
- `refer/wezterm/term/src/terminalstate/mod.rs`：`TerminalState` 字段、`TerminalMode` 位域、`ScreenOrAlt`、边距、alt screen 切换、reflow。
- `refer/wezterm/term/src/terminalstate/performer.rs`：每个动作的具体语义。

**验收**：把 `refer/wezterm/term/src/test/{c0,c1,csi}.rs` 的用例翻译成 vtx 单测；另外加"屏幕 → 文本快照"helper（把可见区拼成 `String` 便于断言）。**这是整个方案里保证正确性的核心手段。**

---

### M5 输入编码

**职责**：egui 的事件 → 写给 pty 的字节。**不含**终端模型（但需要读模式位：应用光标键、bracketed paste）。

**API 草案**

```rust
// crates/vtx-core/src/input.rs
/// 与终端模型无关的输入，便于单测。
pub enum InputEvent {
    Key { key: Key, modifiers: Modifiers, repeat: bool },
    Text(String),
    Paste(String),
}

/// 把一个输入事件编码成一串要写给 pty 的字节。
pub fn encode(event: &InputEvent, modes: &Modes, out: &mut Vec<u8>) -> EncodeResult;

pub enum EncodeResult { Written, Ignored }
```

**实现要点**

1. **`Event::Text` 与 `Event::Key` 的分流**（这是最容易做错的地方）：
   - `Event::Text(String)` 已经包含键盘布局/IME/组合键的最终结果，**普通可打印输入优先用它**，不要自己从 `Key` 拼字符（否则非 US 布局、IME、死键全错）。
   - 但当 `modifiers.ctrl || modifiers.command || modifiers.alt` 成立时，`Event::Text` 仍可能给出 `'a'` 这种**字面字符**，此时必须改走控制序列编码，否则 `Ctrl+A` 会变成 `a`。
   - `Event::Key { key, pressed, repeat, modifiers }` 只处理 `pressed == true`，并且只处理**功能键**（方向/Home/End/Insert/Delete/PageUp/PageDown/F1–F35/Esc/Tab/Enter/Backspace/Space）与**控制组合**。
2. **已核实的 egui 0.36 事件面**：`Event::Key { key, physical_key, pressed, repeat, modifiers }`、`Event::Text`、`Event::Paste(String)`、`Event::Copy`、`Event::Cut`（`egui-0.36.1/src/data/input/event.rs:17`）。`Modifiers { alt, ctrl, shift, mac_cmd, command }`（`data/input/modifiers.rs:19`，注意 `command` 在 win/linux 等于 `ctrl`，在 mac 等于 `mac_cmd`）。`Key` 共有 `ArrowDown/Left/Right/Up`、`Escape`、`Tab`、`Backspace`、`Enter`、`Space`、`Insert`、`Delete`、`Home`、`End`、`PageUp`、`PageDown`、`Copy`、`Cut`、`Paste`、标点（`Colon`、`Comma`、`Backslash`、`Slash`、`Pipe`、`Questionmark`、`Exclamationmark`、`OpenBracket`、`CloseBracket`、`OpenCurlyBracket`、`CloseCurlyBracket`、`Backtick`、`Minus`、`Period`、`Plus`、`Equals`、`Semicolon`、`Quote`）、`Num0`–`Num9`、`A`–`Z`、`F1`–`F35`（`data/key.rs`）。
3. **编码表**（Xterm 风格，第一版只做这一种）：
   - 方向键：普通 `\x1b[A/B/C/D`；`DECCKM`(`?1h`) 打开时 `\x1bOA/B/C/D`。
   - `Home/End`：`\x1b[H`/`\x1b[F`（应用模式 `\x1bOH`/`\x1bOF`）。
   - `Insert/Delete/PageUp/PageDown`：`\x1b[2~` / `\x1b[3~` / `\x1b[5~` / `\x1b[6~`。
   - `F1`–`F4`：`\x1bOP..S`；`F5`–`F12`：`\x1b[15~..24~`；`F13`–`F35` 按 xterm 表继续。
   - `Tab`：`\t`；`Shift+Tab`：`\x1b[Z`；`Enter`：`\r`；`Esc`：`\x1b`；`Backspace`：`\x7f`（xterm 默认，若被 `stty` 改成 `^H` 则发 `\x08` —— 第一版发 `\x7f` 并在注释里标注）。
   - `Ctrl+字母` → `0x01..0x1A`；`Ctrl+Space` → `0x00`；`Ctrl+[`/`Ctrl+]`/`Ctrl+\`/`Ctrl+^`/`Ctrl+_` 按 C0 表。
   - `Alt+字符` → `\x1b` + 字符（wezterm 的 METAv 行为）。
   - `Ctrl+C`：**无选区时发 `0x03`（即 SIGINT）**，有选区时改为"复制到剪贴板"，这是 wezterm 的 `termwindow/keyevent.rs` 逻辑。
4. **粘贴**：`bracketed paste`(`?2004h`) 打开时包 `\x1b[200~ ... \x1b[201~`，否则原样；顺手把 `\r\n`/`\n` 归一化成 `\r`。
5. **鼠标**（推迟到 M9，但 `encode` 的签名先留出位置）：`1006` 模式下是 `\x1b[<b;x;yM`。
6. `KeyboardEncoding`（wezterm 支持 Xterm/Kitty/Win32/WezTerm 多种）第一版只做 Xterm，用 enum 占位。

**wezterm 参考**

- `refer/wezterm/termwiz/src/input.rs`：**M5 的主要参考** —— `InputEvent`/`KeyCode`/`KeyEvent` 以及 `KeyEvent::encode`（把键事件编码成字节的核心表，含修饰键组合、应用模式分支）。
- `refer/wezterm/termwiz/src/keymap.rs`：键名到 `KeyCode` 的映射。
- `refer/wezterm/wezterm-input-types/src/lib.rs`：`KeyboardEncoding`、`KeyCode`、`Modifiers` 的类型定义（vtx 的 `InputEvent` 可以照这个裁剪）。
- `refer/wezterm/wezterm-gui/src/termwindow/keyevent.rs`：**窗口事件 → termwiz `KeyEvent` → 写 pty** 的完整流程，含 "Ctrl+C 有选区则复制" 这类分支 —— egui 世界里对应的就是 vtx 的 M5+M8。
- `refer/wezterm/term/src/terminalstate/keyboard.rs`：应用光标键模式等对编码的影响。

**验收**：纯单测，不需要 PTY。用例：`Ctrl+A`→`[0x01]`、`ArrowUp`（普通/应用模式）、`F5`、`Shift+Tab`、`Alt+x`、bracketed paste 开关两种情况、`Ctrl+C` 有/无选区。

---

### M6 尺寸联动（窗口 → 终端 → PTY）

**职责**：把 egui 面板的像素尺寸变成 `cols/rows`，同时更新终端模型和 PTY。

**数据流**

```
TerminalView::show 拿到 rect 与 cell_size
  cols = floor(rect.width()  / cell_w).max(1)
  rows = floor(rect.height() / cell_h).max(1)
  if (cols, rows) != state.grid_size():
      state.resize(cols, rows)          // M4：裁剪/补行/钳制光标
      pty.resize(PtySize { cols, rows, .. })   // M1
```

**实现要点**

1. 触发点有两个，都要接：
   - `TerminalView::show` 里比较本次 `rect` 与上次记录的 `rect`（**主要路径**，分割条拖动、dock 变化、窗口缩放最终都会反映到 rect）；
   - `egui_dock::TabViewer::on_rect_changed`（已核实存在：`egui_dock-0.21.1/src/widgets/tab_viewer.rs:77`）—— 作为"面板几何变化"的显式信号。
2. **节流**：拖动分割条会每帧产生新尺寸，逐帧 resize PTY 会让 shell 疯狂收到 SIGWINCH。做法：尺寸变化后延迟到"连续 N 帧稳定"或固定 ~50 ms 再下发（记住 pending 尺寸，用 `ctx.request_repaint_after` 驱动）。wezterm 在 `termwindow/resize.rs` 里也有类似的合并逻辑。
3. `cols/rows` 至少为 1，避免 0 尺寸导致除零或 PTY 异常。
4. 减少行时**不要丢掉数据**：顶部行应按边距规则进 scrollback 而不是直接丢（wezterm 的行为）。第一版可以简单地"按边距裁剪"，但要在注释里标出与 wezterm 的差异。
5. **reflow 推迟**：列宽变化时 vtx 只做截断/补齐，wezterm 会重新折行。参考 `refer/wezterm/term/src/terminalstate/mod.rs` 的 reflow 实现，将来单独做。

**wezterm 参考**

- `refer/wezterm/wezterm-gui/src/termwindow/resize.rs`：窗口尺寸 → 网格尺寸 → 各 pane resize 的完整链路。
- `refer/wezterm/wezterm-gui/src/termwindow/render/pane.rs`：单个 pane 的几何（含 padding、像素尺寸计算、`pixel_width`/`pixel_height` 传给 PTY）。
- `refer/wezterm/mux/src/pane.rs` 的 `Pane::resize` 与 `refer/wezterm/mux/src/localpane.rs` 的 `resize`（落到 `master.resize`）。
- `refer/wezterm/term/src/screen.rs` 的 `Screen::resize`、`refer/wezterm/wezterm-surface/src/line/line.rs` 的 `Line::resize`（`will_wrap` 参数语义）。

**验收**：集成测试 —— 拖分割条后 `stty size` 的输出与网格一致（unix）；Windows 下用 `mode con` 或 PowerShell 的 `$Host.UI.RawUI.WindowSize` 目视核对。

---

### M7 渲染改造

**职责**：把 `Screen` 画到 egui 里。保留现有结构（行级 galley 缓存 + run 合并背景 + 光标块），替换字形定位方式。

**先说结论**：推荐走 **逐字形绘制**（下称**路径 A**），并在 M7 第一步用一个 spike 验证其坐标约定；验证不通过则退化为**路径 B**（保留 `painter.galley`）。

#### 路径 A：逐字形（推荐，等价于 wezterm 的做法）

已核实的可行性依据：

- `epaint::text::Galley.rows: Vec<PlacedRow>`，`Row.glyphs: Vec<Glyph>` **公开**（`epaint-0.36.1/src/text/text_layout_types.rs:830` 的 `Row`、`:886` 的 `Glyph`）。
- `Glyph { chr, pos: Pos2, advance_width: f32, line_height, font_ascent, font_height, uv_rect: UvRect }` —— 定位所需信息全在里面。
- `UvRect { offset: Vec2, size: Vec2, min: [u16; 2], max: [u16; 2] }` **字段全公开**（`epaint-0.36.1/src/text/font.rs:22`），其中 `offset`/`size` 是"points 单位、供好看渲染的偏移与尺寸"，`min`/`max` 是纹理里的**纹素**坐标。
- **字体图集就是 `TextureId::default()`** —— 已核实 `egui-0.36.1/src/context.rs:2676`：`tex_mngr.set(TextureId::default(), font_image_delta)`。所以用 `Painter::image(TextureId::default(), ..)` 就能采样字形位图。
- `Fonts::font_image_size() -> [usize; 2]` 公开（`epaint-0.36.1/src/text/fonts.rs:780` 与 `:844`），用于纹素 → 归一化 UV。
- `Painter::image(TextureId, Rect, uv: Rect, tint: Color32)` 与 `Painter::rect_filled` 都是公开 API（`egui-0.36.1/src/painter.rs:447`）。

绘制配方：

```text
每个可见行 row:
  1. 背景: 按 run（chunk_by(相同 bg)）画 rect_filled        // 已有实现，保留
  2. 文本:
     - 取该行的 galley（缓存命中则复用）
     - for glyph in galley.rows[0].glyphs:
           col   = 该字形所属单元格的列号        // 关键：用 cell 的 x，不用 glyph.pos.x
           dest  = Rect::from_min_size(
                       pos2(origin.x + col * cell_w + glyph.uv_rect.offset.x,
                            row_top  + glyph.uv_rect.offset.y),
                       glyph.uv_rect.size)
           uv    = Rect::from_min_max(
                       pos2(min[0] as f32 / atlas_w, min[1] as f32 / atlas_h),
                       pos2(max[0] as f32 / atlas_w, max[1] as f32 / atlas_h))
           painter.image(TextureId::default(), dest, uv, fg_color_of_that_cell)
  3. 装饰: underline / strikethrough / overline 用 rect_filled 画
  4. 光标: 按 cursor_shape 画 Block / Bar / Underline
```

**为什么这样就"格位正确"**：水平位置由**我们算的单元格 x**（`col * cell_w`）决定，而不是由 shaping 的自然 advance 累积决定 —— 双宽 CJK 严格占 2 格，窄字形严格占 1 格，`grapheme` 簇内的组合字由 harfrust 负责成形，基线由 `uv_rect.offset.y` 对齐到行顶。这正是 wezterm 的做法（它自己管理 glyph 位置与图集）。

**M7 第一步必须先做的 spike（务必做）**：写一个一次性测试/示例，把一行 `"a中b"` 的 galley 打印出来（`galley.rows[0].glyphs` 的 `pos`、`advance_width`、`uv_rect`），确认：

- `uv_rect.offset` 的参照原点是"pen 位置"还是"行顶"；
- `glyph.pos.y` 是否仍是 `f32::NAN` —— **已核实**在原始 shaping 里是 NaN（`epaint-0.36.1/src/text/text_layout.rs` 的 `ShapingContext::glyph`：`pos: pos2(physical_x / pixels_per_point, f32::NAN)`，y 在后续行布局阶段才填），所以**纵向一律用 `uv_rect.offset.y` 相对行顶**，不要用 `glyph.pos.y`；
- 一个 run 里首个字形的 `pos.x` 是否等于 0（用于把 glyph 映射回列号）。

映射 glyph → 列号建议用"每单元格一个 run + 记录每个 run 的起始列"的方式，避免反推。

#### 路径 B：保留 `painter.galley`（退化方案）

不动定位方式，只做两项改进：

1. `TextFormat.line_height = Some(cell_h)`（**已核实**生效方式：`section.format.line_height.unwrap_or(font_metrics.row_height)`，`text_layout.rs:446`）—— 让每行正好占 `cell_h`，网格与基线对齐。
2. 用 `TextFormat.extra_letter_spacing` 在 run 内做补偿，让窄字形的总宽接近 `cell_w` 的整数倍。**注意其语义已核实**（`text_layout.rs:282`）：只在**簇边界**施加，且**section 的第一个簇不加**，因此**无法**给 section 的末字形补间距 —— 这就是"每单元格一个 section + 用 spacing 对齐"这个取巧方案不成立的原因，别走这条路。

路径 B 下双宽字形仍会有轻微错位，需在代码注释里标为已知偏差。

#### 两条路径共用的改造

3. **行缓存的 key 换成 `(stable_row_index, seqno)`**（配合 M4），不再假设"第 N 行"是稳定的。缓存 `Arc<Galley>`。
4. **装饰用 rect 画**，不用 `TextFormat.underline`/`strikethrough`：逐字形绘制时没有行级 `TextFormat`，而且 rect 才能精确跨双宽字符的 2 格宽度（wezterm 也是画 rect）。
5. **光标**：`CursorShape::{Block, Bar, Underline}` + `DECTCEM` 可见性；闪烁用 `ctx.request_repaint_after(...)`（不要每帧 `request_repaint`，那是空转）。窗口失焦时 wezterm 画空心块，可以照做。
6. **反显（DECSCNM）**：整屏 fg/bg 互换，在 `foreground_of`/`background_of` 里加一层判断即可（现有代码已经有 reverse 视频的交换逻辑）。
7. 选择性优化（推迟）：字形级缓存（对应 wezterm 的 `glyphcache.rs`）。第一版行级 galley 缓存已经够用。

**wezterm 参考**

- `refer/wezterm/wezterm-gui/src/termwindow/render/screen_line.rs`：**一行怎么画** —— run 切分、背景、字形、下划线/删除线，逐字形 quad 的位置与 UV 怎么算。M7 的核心参考。
- `refer/wezterm/wezterm-gui/src/termwindow/render/mod.rs`、`.../render/paint.rs`、`.../render/draw.rs`：paint pass、quad 列表的组装顺序（背景 → 字形 → 装饰 → 光标）。
- `refer/wezterm/wezterm-gui/src/glyphcache.rs`：字形图集与缓存 key（`GlyphKey`：码点 + 字体 + 字号 + 表现形式 + 布局上下文）。
- `refer/wezterm/wezterm-gui/src/termwindow/mod.rs`：`invalidate()` / `NeedRepaint` 的脏行机制 —— 对 vtx 就是"哪些行要重 shape"。
- `refer/wezterm/wezterm-surface/src/cellcluster.rs`：`CellCluster` 的 run 切分规则。

**验收**：肉眼 + 截图；具体用例：`printf '\e[31mred\e[0m\n'`、CJK（`中`）、宽字符与半角混排、行内下划线跨宽字符、光标三种形状、alt screen 切换（`vim`/`less`）后无残影。

---

### M8 egui_dock 集成与进程生命周期

**职责**：标签/分屏管理、把 UI 事件转给 M5/M1、把尺寸变化转给 M6、**关闭时正确杀进程**。

**实现要点**（全部基于已核实的 egui_dock 0.21 `TabViewer` 接口：`egui_dock-0.21.1/src/widgets/tab_viewer.rs:6`）

1. `Pane` 从"只有一个 `TerminalView`"升级为持有 `Arc<Mutex<TerminalState>>` + `Box<dyn Write + Send>`（writer）+ `Box<dyn ChildKiller + Send + Sync>` + reader 线程句柄。
2. **`on_close(&mut self, tab) -> OnCloseResponse`**（`:35`，`enum OnCloseResponse { Close, Focus, Ignore }` 在 `:113`）：第一版直接 `killer.kill()` 后返回 `Close`。若子进程尚未退出，可以先返回 `Ignore` 并等 `wait()` 线程回报 —— wezterm 的 `KillPane`/`ClosePane` 语义就是"能优雅则优雅，否则强杀"（unix 下 SIGHUP → 超时 → SIGKILL）。
3. **`is_closeable` / `force_close`**（`:42` / `:59`）：`force_close` 返回 `true` 时 egui_dock 会直接移除标签**而不调 `on_close`** —— 所以所有资源释放都不能只放在 `on_close` 里，必须由 `Pane` 的 `Drop` 兜底（`Drop` 里 `kill()`）。
4. **`on_rect_changed(&mut self, rect: Rect, tab: &mut Tab)`**（`:77`）：接 M6 的尺寸联动。
5. `on_add`（现有实现）改为"创建 pane 时真的 `spawn` 一个新 pty"。
6. **应用退出**：`impl Drop for TerminalTab` 遍历所有 pane kill（或 kill 所有 killer）。防止孤儿 shell 进程。
7. **每帧的固定顺序**（在 `TerminalView::show` 里）：
   1. `ui.input(|i| i.events.clone())` 取事件 → 过滤出本 pane 的事件（键盘事件给聚焦 pane，由 egui 的 `Response.has_focus`/`Sense::click` 决定）→ `encode` → 写 writer；
   2. 取出 `TerminalState.responses` 写 writer（DSR/DA 应答）；
   3. 绘制；
   4. 尺寸检查（M6）。
8. 焦点与事件：终端要 `ui.allocate_exact_size(size, Sense::click_and_drag())` 并 `response.request_focus()`，否则收不到键盘事件。注意 egui 的 `Event::Text` 会同时送给"获得焦点的 widget"，要避免被别处消费。
9. 子进程退出：`wait()` 线程回报后置 `TerminalState.exited`，标签标题改成"已退出"（wezterm 显示 "Process exited" 并保留最后画面）；此时 `force_close` 返回 `true` 让标签可被直接移除。
10. `title`（M4 的 OSC 0/2）推给 egui：`ctx.send_viewport_cmd(ViewportCommand::Title(s))`（**已核实** `ViewportCommand::Title(String)` 存在，`egui-0.36.1/src/viewport.rs:1090`），并同步到标签标题（`TabViewer::title`）。

**wezterm 参考**

- `refer/wezterm/mux/src/pane.rs`：`Pane` trait —— `writer`/`resize`/`kill`/`wait`/`is_dead`，vtx 的 `Pane` 就是它的具体化。
- `refer/wezterm/mux/src/localpane.rs`：本地 pane 的 spawn/reader/child 管理。
- `refer/wezterm/mux/src/window.rs`：`Window` 持有 pane 集合、关闭时的资源释放。
- `refer/wezterm/mux/src/tab.rs`：标签模型（vtx 用 egui_dock 的 `DockState` 代替）。
- `refer/wezterm/wezterm-gui/src/termwindow/mod.rs`：`close_pane`/`window_closing` 的清理路径。
- `refer/wezterm/wezterm-gui/src/termwindow/spawn.rs`：新建 pane 时的 spawn 流程。

**验收**：开 3 个标签 → 关 1 个 → 无残留进程（win `Get-Process`，unix `ps`）；在一个标签里 `exit` → 该标签显示已退出且不崩；关闭整个应用 → 所有 shell 退出。

---

## 5. 如何保证正确性（本方案的核心投资）

VT 语义的正确性不能靠肉眼。建议：

1. **把 wezterm 的测试翻译过来**（最高性价比）：
   - `refer/wezterm/term/src/test/mod.rs`：测试脚手架（构造终端、喂字节、断言屏幕）—— 照它做一个 vtx 的 `test_support`；
   - `refer/wezterm/term/src/test/c0.rs`：`BS`/`HT`/`LF`/`CR`/`RI`/`NEL` 等 C0 行为；
   - `refer/wezterm/term/src/test/c1.rs`：`IND`/`NEL`/`HTS` 等 C1 行为（**顺带验证 M2 里提到的 UTF-8 编码 C1 的处理**）；
   - `refer/wezterm/term/src/test/csi.rs`：`CUU/CUD/CUF/CUB/CHA/CUP/ED/EL/IL/DL/ICH/DCH/ECH/SGR/DECSTBM` —— 覆盖 M3+M4 的主体；
   - `refer/wezterm/term/src/test/selection.rs`、`test/image.rs`：**跳过**（超出范围）。
2. **屏幕快照 helper**：把可见区 + 光标位置拼成多行 `String`，断言起来直观且 diff 可读。
3. **对拍**：手工跑 `printf` 转义序列与 `vttest`（若环境可用），和 wezterm / Windows Terminal 目视比较。
4. **分层测试**：M3 用纯函数测试（不建 PTY），M1/M6/M8 才需要集成测试。

---

## 6. 需要拍板的横切决策（含推荐）

| 议题 | 推荐 | 理由 |
| --- | --- | --- |
| `tokio` 依赖 | **移除** | `portable-pty` 是阻塞 API；用 `std::thread` + `std::sync::mpsc`。wezterm 也是 std thread + smol 通道 |
| 线程模型 | **reader 线程内直接 parse**，单线程 | wezterm 用 socketpair 把 read 与 parse 分成两个线程，是为多窗口/多路复用下的锁竞争优化的；vtx 单窗口不需要，少一次拷贝 |
| 行存储 | **只做稠密 `Vec<Cell>`** | 已有；wezterm 的稀疏 `ClusteredLine` 是内存优化，vtx 暂不需要 |
| 渲染 | **路径 A（逐字形）**，先做 spike 验证；不通过退路径 B | 格位严格对齐（双宽 CJK），且与 wezterm 思路一致 |
| `SequenceNo` 类型 | 保持 `u64` | 已是；wezterm 用 `usize`（`wezterm-surface/src/lib.rs:90`），无实质差异 |
| 模式位实现 | 手写位域 | 与 [cell.rs](crates/vtx-core/src/cell.rs) 的 `CellAttrs` 风格统一，不引入 `bitflags` |
| 新增依赖 | `unicode-segmentation`（grapheme 切分） | vtx-core 需要；`unicode-width` 已在 |
| reflow | **第一版不做**，代码注释标明 | wezterm 的实现规模不小，先让主链路跑通 |
| 默认 shell | unix `$SHELL` → `/bin/sh`；win `COMSPEC` → `powershell.exe` | 与 wezterm 的默认行为一致 |

**编码约定**：工作区 lint 是 `missing_docs = "warn"` 与 `unsafe_code = "forbid"`，所以每个公开项都要有 `///` 文档，且不能有 `unsafe` —— 这正好与 wezterm 的重 `unsafe`（unix fork/exec）形成对比：vtx 靠 `portable-pty` 把这些都挡在了依赖里。

---

## 7. 里程碑

| 里程碑 | 内容 | 可独立验证 |
| --- | --- | --- |
| S1 | M1 + M2 + M3（PTY 能起、能读、能解析成 `Action`） | 起一个 shell 并把 `ls` 的输出打成 `Action` 日志 |
| S2 | M4（屏幕模型 + c0/c1/csi 测试全绿） | 纯单测；这是最重的一段 |
| S3 | 把 M1+M4 接起来，`TerminalView` 显示真实 shell 输出（暂用路径 B 渲染） | 手动敲 `ls` 能看到结果 |
| S4 | M5（输入） | 可以真的交互（敲命令、Ctrl+C、方向键调历史） |
| S5 | M6 + M7（尺寸联动 + 逐字形渲染） | 拖窗口不花屏，CJK 对齐 |
| S6 | M8（多标签 + 进程生命周期） | 开/关标签无残留进程 |

S1 与 S2 可以并行（M2/M3 是纯函数）。**建议从 S2 的 M4 开始**，因为它是全局瓶颈，而且它的测试用例（wezterm 的 test 目录）是现成的。

---

## 8. 横向对比：wezterm vs Konsole vs VTX

本节把两个参照项目放在同一张桌子上，目的是回答一个问题：**VTX 的每个模块该抄谁、为什么**。wezterm 是"新派的 Rust + GPU"路线，Konsole 是"老派的 Qt/C++ + 立即模式绘制"路线，两者在同一个问题上经常给出相反的答案 —— 这些分歧点比任何单一实现都更有参考价值。

### 8.1 两个参照项目的形态

| 维度 | wezterm | Konsole |
| --- | --- | --- |
| 语言 / 工具链 | Rust（edition 2018），无 GC，`unsafe` 用于 unix 平台代码 | C++20 / Qt 6.5+ / KF6 6.6+ |
| 项目性质 | 单人主导（Wez Furlong），一个仓库同时是终端 + 多路复用器 + SSH 客户端 | KDE 官方项目，多人长期维护，纯终端（内嵌与远程交给别的组件） |
| 渲染方式 | 自绘 GPU 管线：quad/atlas，自己算字形位置与 UV | `QPainter` 立即模式逐格 `drawText`，无图集、无字形缓存 |
| 配置方式 | 单个 Lua 配置文件（可编程） | Profile / `.colorscheme` / `.keytab` 数据文件 + 图形配置对话框 |
| 可嵌入性 | 独立应用；通过内建 mux server 让多个客户端 attach 同一会话 | 自身是 KParts 组件（`src/Part.h`），可被 Kate / KDevelop / Yakuake 直接嵌入 |
| 多路复用 / 远程 | 内建（`mux` crate），SSH 是第一等公民 | 无（另有 ZModem 钩子、容器检测等终端侧集成） |
| 跨平台 | unix + Windows（自己的 ConPTY 绑定） | unix + Windows（`src/ptyqt/` 的 ConPTY 绑定） |
| 测试方式 | 单测内嵌在各 crate（`term/src/test/` 约 2.1k 行）+ `vtparse` 自带测试 | QTest autotests（`src/autotests/` 约 5.4k 行）+ `tests/` 手工用例集 |
| 版本基线 | master 快照 | 26.11.70（开发快照） |

**一句话概括这两个项目的路线差异**：wezterm 把终端当作一个**要自己掌控每一层的系统**（所以它愿意写 GPU 渲染器和多路复用器，也愿意为此维护一套 10k 行的转义序列语义层）；Konsole 把终端当作**一个 Qt 控件**（所以它直接用 `QPainter` 画字、用 `QWidget::scroll` 滚动像素、用事件循环做 I/O）。VTX 用 egui 复刻，天然更接近 Konsole 的"控件"定位，但在解析层和屏幕模型上 wezterm 的抽象更干净。

### 8.2 规模对照（实测）

按模块统计行数（含注释与空行；统计的是下列文件/目录）：

| 关注点 | wezterm | Konsole |
| --- | --- | --- |
| 转义序列解析（字节级状态机） | `vtparse/` 1 601 | — （与下一行合并在 `Vt102Emulation`） |
| 转义序列语义层 | `wezterm-escape-parser/` 10 872 | `Vt102Emulation.cpp/.h` 4 918 |
| 屏幕模型 / 行缓冲 | `wezterm-surface/` 5 242 | `Screen.cpp/.h` 3 737 + `ScreenWindow` 633 |
| 终端状态机（模式/光标/动作） | `term/` 10 213 | 含在 `Vt102Emulation` 与 `Screen` 中 |
| PTY + 会话 | `pty/` 2 477（+ `mux/localpane` 等） | `Pty.cpp/.h` + `session/Session.cpp/.h` 4 573 |
| 多路复用 | `mux/` 11 757 | 无对应物 |
| 渲染 | `wezterm-gui/src/termwindow/render/` 4 502（另有 `glyphcache.rs` 等未计入） | `TerminalDisplay.cpp/.h` 4 527 + `TerminalPainter` 1 428 |
| 单元格 / 属性 / 宽度 | 含在 `wezterm-cell`、`wezterm-surface` | `characters/` 2 364 |
| 回滚缓冲 | 含在 `term/` | `history/` 970 |
| 屏幕 → 文本导出 | 含在 `term/`（selection） | `decoders/` 451 |
| 键盘编码 | `wezterm-input-types` + `termwiz/input.rs` | `keyboardtranslator/` 749 + `data/keyboard-layouts/*.keytab` |
| 测试 | `term/src/test/` 2 137 | `src/autotests/` 5 414 |
| **全项目（源文件）** | — | `src/**.cpp/.h` 70 116（314 个文件） |
| **VTX 现状** | `crates/vtx-core` 1 010 + `crates/vtx-ui` 688 | |

三条对 VTX 有用的观察：

1. **解析层是最贵的一块**。wezterm 的 `vtparse` + `wezterm-escape-parser` 合计约 12.5k 行，是 Konsole `Vt102Emulation` 的 2.5 倍。差距主要来自 wezterm 把序列建模成结构化的 `Action`（带分配、带扩展协议），而 Konsole 是在一个大 switch 里直接改 `Screen`。**VTX 走的是 wezterm 的路线（M2 + M3 分离），所以要预期 M3 是行数最多、最需要测试驱动的模块。**
2. **只做基础 VT 的规模预期**。把两边与 VTX 范围相当的部分加起来：Konsole 侧 `Vt102Emulation` + `Screen` + `ScreenWindow` + `TerminalDisplay` + `TerminalPainter` + `characters` + `history` + `KeyboardTranslator` + `Pty` ≈ **2 万行 C++**；wezterm 侧扣掉多路复用、SSH、图片协议、鼠标上报等高级特性后 ≈ **2.5–3.5 万行 Rust**。即 VTX 要做到基础 VT 的功能对等，大概是一个 **1.5–2.5 万行**的项目（当前 1.7k）。这个数量级值得在动手前就知道 —— 它不是"一周的玩具"。
3. **Konsole 的测试占比更高**（5.4k / 70k ≈ 7.7%，且集中在 `Vt102Emulation`/`Screen` 这些核心上）。它的测试组织方式见 §8.4，与我们在 §5 里"把 wezterm 的 test 目录翻译过来"的策略可以互补：**wezterm 提供语义的权威定义，Konsole 提供"如何为终端写单测"的工程范式。**

### 8.3 逐模块三方对照

| 模块 | wezterm | Konsole | VTX 的选择 |
| --- | --- | --- | --- |
| **M1** PTY / 生命周期 | 自带 `pty` crate（unix fork/exec + 自写 ConPTY）；`ChildKiller` 显式杀进程；每 pane 一个专用 reader 线程（1 MiB 缓冲），再经 socketpair 交给解析线程 —— **两个线程** | Unix 直接继承 KDE 的 `KPtyProcess`（fork/exec/控制终端都在框架里，本仓库不做）；读取走**事件循环**：`KPtyDevice::readyRead` → `Pty::dataReceived` → `readAll()`，**无固定缓冲、无线程**；Windows 反而用专用读线程（`ReadFile` 1024 字节循环） | 取 wezterm 的线程模型（egui 没有 Qt 那种与 fd 集成的 IO 事件循环，且我们需要一个自然的 `request_repaint` 触发点），用 `portable-pty` 承担 `KPtyProcess` 的角色 |
| **M2+M3** 解析与语义 | `vtparse` 只管状态机（1.6k）；`wezterm-escape-parser` 把回调转成结构化 `Action`（10.9k，含大量扩展协议）。先收集成 `Vec<Action>` 再整批 apply | `Vt102Emulation` 自己实现 DEC 的**表驱动状态机**（`charClass[256]` + 状态枚举），参数打包成 `token_csi_*`，最后进一个约 2 400 行的 `processToken` 大 switch，**直接改 `Screen`**；状态机与语义合为一体（4.9k） | 走 wezterm 路线：`vtparse` 负责状态机，M3 手写语义层 —— 语义层可纯函数单测、可事务化（脏行一次算完） |
| **M4** 屏幕模型 | `wezterm-surface` 的 `Line` 有稠密/稀疏两种实现；`term` 是状态机。行有稳定标识（`SequenceNo`），alt screen 是 `ScreenOrAlt` | 每行是**变长** `QVector<Character>`（行长存在 `LineProperty.length`，只在 `getImage()` 时才展平成定长网格）；scrollback 是可插拔的 `HistoryScroll`（内存/文件/禁用）；**没有稳定行号** | 定长稠密 `Vec<Cell>`（现状）+ 引入稳定行号（抄 wezterm）。scrollback 先用内存 `VecDeque`；Konsole 的**文件后端**是个值得留意的后续选项 |
| **M5** 输入编码 | `termwiz::input::KeyEvent::encode`，规则**硬编码在代码里**，按 `KeyboardEncoding`（Xterm/Kitty/Win32/WezTerm）分支 | **数据驱动**：`.keytab` 文本文件（`data/keyboard-layouts/{default,linux,macos,solaris}.keytab`）描述"键 + 修饰键 + 模式 → 字节序列"，用户可用 `KeyBindingEditor` 自行编辑；模式（NewLine / Ansi / DECCKM / AppScreen / AppKeyPad）作为 **state 标志**参与匹配，所以模式相关的编码差异也写在数据里。**未命中的键回退到 Qt 给的文本**（`_encoder.encode(event->text())`）—— Ctrl+C 的 `\x03` 不是 keytab 条目，而是这条回退路径产生的 | 取 wezterm 的硬编码路线（第一版只需 Xterm 一种），但把 Konsole 的**回退原则**吸收进 M5 的分流规则：`Event::Text` 优先、`Event::Key` 只管功能键与功能组合 —— Konsole 的"未命中就用原始文本"正是这个意思 |
| **M6** 尺寸与 reflow | 有 reflow；窗口 → `Pane::resize` → `MasterPty::resize`，逻辑在 `termwindow/resize.rs` | 有 reflow 且**默认开**（`Profile::ReflowLines`）：`Screen::resizeImage` 合并 wrapped 续行、拆分超宽行、在屏幕与 history 间迁移行，上限 20 000 行。resize 链是 view → `Session::updateTerminalSize`（多个 view 时取**最小**行列）→ `Emulation::setImageSize` → `Pty::setWindowSize`（`TIOCSWINSZ`）；**pty 侧无防抖** | 第一版不做 reflow（§6 已定），但架构上要留（稳定行号是 reflow 后重新定位的基础）；同时**要有防抖**（Konsole 没有，拖分割条会让 `TIOCSWINSZ` 连发） |
| **M7** 渲染 | 自绘 GPU：字形 shape 成 cluster，按 cell 摆 quad，位图进 atlas；脏行 → invalidate → paint；行级 quad cache | `QPainter` 立即模式，**逐格** `drawText`（只有 wordMode 会把同词类的连续格合成一次调用）；位置 `leftPadding + fontWidth * col` —— **由列号推出，不累加 advance**；无图集、无字形缓存；靠 `dirtyMask` + `QRegion` 只重绘变化行；滚动时 `memmove` 图像 + `QWidget::scroll` 位块传送 | 位置同样由列号推出（两者一致）→ 采用 M7 路径 A；但把字形贴图交给 egui 的字体图集（`Painter::image`），介于 Konsole 的"无缓存"与 wezterm 的"自建 atlas"之间 |
| **M8** 会话 / 标签 / 生命周期 | `mux` 的 `Pane` / `Window` / `Tab` 三层，pane 是 `Arc<dyn Pane>`；因为内建 mux server，一个会话可被多个客户端 attach | `ViewManager` 持 `TabbedViewContainer`（QTabWidget），每个 tab 内是 `ViewSplitter`（QSplitter）装多个 `TerminalDisplay`；`_sessionMap` 把 display 映射到 session；关 tab → 销毁 display → `viewDestroyed` → **该 session 再无 view 时才 `session->close()`**（SIGHUP → closePty → 超时 → SIGKILL）；**支持 detach/reattach**：把活着的 session 连同 splitter 移进新的 `MainWindow`；进程自行退出时若 `_autoClose` 则连 view 一起删，否则标题改 "Finished" 并保留标签 | 用 egui_dock 的 `DockState` 一体承担 Konsole 里 `ViewManager` + `TabbedViewContainer` + `ViewSplitter` 三者的职责（标签/分屏/拖动都现成）；但抄 Konsole 两点：**①"最后一个 view 关闭才关会话"的引用计数**（VTX 目前 pane:session 是 1:1，暂时用不上，将来一个会话多视图时必须补）；**②进程退出后不立即删标签，而是改标题保留最后画面**（wezterm 亦同） |

### 8.4 八处分歧的纵深比较

这些是两家给出**相反答案**的地方，也是 VTX 必须自己表态的地方。

**① 读取模型：专用线程 vs 事件循环。**
wezterm 每个 pane 一个阻塞 read 线程（1 MiB 缓冲），并且在解析前先经 socketpair 把数据交给另一个线程 —— 这是为多窗口/多路复用下的锁竞争做的优化，代价是每个 pane 两个线程加一次跨线程拷贝。Konsole 在 Unix 上**一个线程都不加**：`KPtyDevice::readyRead` 是 Qt 事件循环里的信号，`Pty::dataReceived` 直接 `readAll()`，然后同步走完 `Session::onReceiveBlock` → `Emulation::receiveData` → 解析 → 更新 `Screen`，重绘则靠 10 ms / 40 ms 两个单发定时器合并（`Emulation::bufferedUpdate`）。

**VTX 取哪边**：取 wezterm 的专用线程。原因是环境不同而非设计更好 —— Qt 的事件循环天生能监听 fd，egui/eframe 不能；而且 VTX 需要在"解析完成"这个时刻唤醒 UI，专用线程里直接 `ctx.request_repaint()` 是最自然的落点。但 Konsole 的**合并定时器**值得学：VTX 不该在每次 read 后都请求重绘，而应让 egui 的帧节奏自然合并（这等于免费拿到了 `bufferedUpdate` 的效果）。

**② 解析产物：结构化 Action vs 大 switch 直改 Screen。**
这是两个项目最本质的架构差异。wezterm 花 10.9k 行把转义序列建模成 `Action` 枚举（`Print`/`CursorMove`/`EraseInDisplay`/`SetAttributes`/…），解析与执行彻底分离；Konsole 在一个 `processToken` 的 switch 里把 2 400 个 case 直接写成对 `Screen` 的方法调用。

分离的代价是行数与分配，收益是：语义可纯函数单测、可跨线程传递、可在一个事务里合并执行。**VTX 取 wezterm**，理由是 §5 的正确性策略依赖它 —— 把 wezterm 的 `term/src/test/` 用例翻译过来时，测的就是"一串 Action 作用在 Screen 上"。

**③ 延迟换行：显式标志 vs 借用光标位置。**
wezterm 用一个显式的 `wrap_next` 布尔标志表示"已在最后一列、下次打印要先换行"。Konsole **没有这个标志**：它让光标被允许停在最后一列的**右边一格**（`_cuX` 可以等于 `columns`），真正的换行发生在其后第一个可打印字符到来时 —— `Screen.cpp` 的注释把理由说得很直白：*"VT100 does wrapping BEFORE putting the character"*。

两种写法都能正确实现延迟换行，但**显式标志更容易推理**（光标位置是"用户可见状态"，被上屏/查询序列读取时不该带上"待换行"这个隐藏含义）。**VTX 取 wezterm 的显式标志。**

**④ 稳定行号：有 vs 无（以及 Konsole 的替代方案）。**
wezterm 给每行稳定标识（`SequenceNo` / `stable_row_index`），滚动时不重编号，脏行追踪和渲染缓存都建立在它之上。Konsole **完全没有这个概念** —— 行按整数位置寻址，`CompactHistoryScroll::_indexBias` 只是内部偏移、不是对外 ID。

Konsole 凭什么不需要？因为它的"缓存"就是**那个图像缓冲本身**：`TerminalDisplay` 持有展平的 `Character* _image`，每次 `updateImage` 把新的屏幕内容与它逐格比较（`dirtyMask[x] = newLine[x] != currentLine[x]`），变化的行进 `QRegion` 重绘，滚动时干脆 `memmove` 缓冲 + `QWidget::scroll` 做位块传送。**用全量 diff 加位块传送，绕过了"行会移动"这个问题。**

**VTX 取 wezterm**，而且这一条是硬性的：egui 没有位块传送（每帧重建 mesh），VTX 又不该每帧全量 diff + 重新 shape 所有行。有了稳定行号，渲染缓存可以用 `(stable_row_index, seqno)` 做 key，滚动的行自然继续命中缓存。**这正是 §1 里指出 vtx 当前 `scroll_up` 后重刷全部 seqno 那个 hack 的正解。**

**⑤ reflow：两家都有，VTX 第一版不做。**
Konsole 的 reflow 默认开启（`Profile::ReflowLines = true`），实现是 `Screen::resizeImage`：先合并标了 wrapped 的续行、再拆分超宽行、再在屏幕与 history 之间迁移行（上限 20 000 行），history 侧还要返回行号偏移 `deltas` 让上层校正。wezterm 也有等价实现。

值得注意的是 **reflow 又一次说明了稳定行号的必要性**：Konsole 因为没有稳定 ID，reflow 后必须额外返回 `deltas` 给调用方去修补位置，而 wezterm 的稳定行号让这件事自然很多。VTX 第一版不做 reflow（§6），但**稳定行号现在就要做** —— 它是将来加 reflow 的前提，而不是与 reflow 一起做的可选优化。

**⑥ 单元格存储：稀疏 vs 定长稠密。**
wezterm 的 `Line` 有稠密（`VecStorage`）和稀疏（`ClusteredLine`）两种实现；Konsole 更彻底 —— 每行就是**变长**的 `QVector<Character>`，真实长度记在 `LineProperty.length` 里，只有 `getImage()` 才展平成定长网格。两者都是为了省内存（行尾大片空白不占空间）。

**VTX 保持定长稠密**（现状）。理由是简单、可直接索引，而 §8.2 的规模预期说明 VTX 的瓶颈在语义正确性而不是内存；真到了要省内存的时候，`Line` 的接口（`write_cell`/`resize`/`cells()`）已经把存储细节封装住了，换实现不算伤筋动骨。

**⑦ 输入编码：硬编码 vs 数据驱动。**
wezterm 把编码规则写死在代码里（`termwiz/src/input.rs` 的 `KeyEvent::encode`），靠 `KeyboardEncoding` 枚举区分协议；Konsole 把它做成**用户可编辑的数据文件**（`.keytab`），连"应用光标键模式下方向键变成 `\EOA`"这种模式差异都由 keytab 条目表达 —— 代码只负责把终端模式翻译成 state 标志位传给匹配函数（`KeyboardTranslator::findEntry`），匹配本身在数据里。

数据驱动的代价是匹配逻辑复杂（`Entry::matches` 要处理 `+mod` / `-mod` 要求、以及"任何修饰键按下即隐含 AnyModifierState"），收益是不改代码就能适配键盘布局与习惯（Konsole 因此能支持 `solaris.keytab` 这种冷门布局）。

但**真正值得 VTX 抄的不是数据驱动，而是那条回退**：Konsole 的 keytab **只描述特殊键**，普通可打印字符与 `Ctrl+字母` 的组合全部回退到 Qt 提供的文本（`_encoder.encode(event->text())`）—— 所以 `Ctrl+C` → `\x03` 根本不是一条 keytab 条目。这与 M5 定的分流规则是同一个原则：**能拿到"用户实际想输入的字符"时就不要自己拼**，只有功能键和功能组合才走编码表。VTX 取硬编码（第一版只需 Xterm），但这条回退原则要显式保留。

**⑧ 测试策略：两家都在"喂字节 → 断言屏幕"，但可见性边界不同。**
Konsole 的核心测试**不需要 GUI 环境**：`Vt102EmulationTest` 与 `ScreenTest` 都用 `QTEST_GUILESS_MAIN`（连 `QApplication` 都不建）。其中 `Vt102EmulationTest` 的做法尤其值得照搬 —— 它用一个 `TestEmulation : Vt102Emulation` 子类，把 `sendString` 换成记录器（记录回写字节 `lastSent`）、把 `processToken` 换成记录器（记录解析出的 token），然后 `sendAndCompare` 喂一段原始字节，**同时断言屏幕内容与回写字节**。`ScreenTest` 更直接：构造 `Screen`、调 `displayCharacter` / `resizeImage`、断言 `selectedText(PlainText)`。六个测试里只有 `PtyTest` 真的起进程（还只是 `sh` 一下），`SessionTest` 只断言默认状态，`SessionManagerTest` 是个空壳。

对照 §5 可得一个具体结论：**VTX 应该把"屏幕 → 文本"的能力做进 `Screen` 本身，而不是只做在测试里。** 依据有二：① Konsole 的 `Screen::text()` / `selectedText(PlainText)` 就是那个快照 helper，测试直接用它；② Konsole 把这个能力抽成了独立模块 `src/decoders/`（`PlainTextDecoder` / `HTMLDecoder` / `TerminalCharacterDecoder`），因为 M8 的复制、以及将来"保存历史输出"都要复用同一套逻辑。VTX 可以在 M4 里先给 `Screen` 一个 `to_text()`（测试用），把 decoder 抽象留到需要复制功能时再拆。

另外，Konsole 除自动单测外还有一份**手工用例集**（`refer/konsole/tests/README.md`），内容对 VTX 可直接复用：`text-files/` 里有 emoji、制表框线、UTF-8 压力测试（Markus Kuhn 的 `UTF-8-demo.txt`、Kermit 的 `GLASS.utf8.txt`）、以及 `bugs/` 下专门用来触发问题的脚本（无限长行、无限组合字符、写满整行触发的 reflow 崩溃、尽可能快地刷 stderr）。这些文件都是"喂进去看结果对不对"的输入，正好给 M3/M4 当回归素材 —— 比自己编用例省事，也更接近真实程序的输出。

### 8.5 结论：VTX 逐个模块抄谁

**抄 wezterm（架构分层）**

- M2/M3 解析与语义分离、`Vec<Action>` 事务化 —— 换取可测试性，§5 的用例翻译策略依赖它；
- M1 每 pane 一个专用 reader 线程 —— egui 没有 Qt 的事件循环，这是最自然的落点；
- M4 **稳定行号** —— 对 VTX 是硬需求（见 ④），不是可选优化；
- M3 **显式 `wrap_pending` 标志** —— 比借用光标位置好推理（见 ③）；
- M7 行缓存 key 用 `(stable_row_index, seqno)`。

**抄 Konsole（工程范式与交互细节）**

- M5 的**回退原则**：能拿到实际输入文本就不自己拼字符（见 ⑦）；
- M7 的**格位由列号推出** —— 两家在这里完全一致，这是 M7 路径 A 最直接的依据；
- M8 的"**最后一个 view 关闭才关会话**"引用计数，以及"**进程退出保留标签、改标题**"；
- §5 的**纯单测范式**：核心逻辑的测试不需要 UI 环境（Konsole 用 `QTEST_GUILESS_MAIN` 表达这件事），断言"喂字节 → 屏幕文本 + 回写字节"；
- 把"屏幕 → 文本"做成 `Screen` 的能力 / 独立 decoder，供测试与复制共用。

**两家都不抄（VTX 的处境决定的）**

- **Konsole 的"全量 diff + `QWidget::scroll` 位块传送"不适用** —— egui 每帧重建 mesh，没有位块传送，所以 VTX 必须靠稳定行号 + 按行 seqno 做增量（这正是 ④ 的结论）；
- **Konsole 的 `setWriteable`（对 tty 设备节点 chmod）、ZModem 钩子、DBus 会话 API、容器检测（distrobox / toolbox）、KParts 嵌入、Profile 与配色方案文件、书签、打印、壁纸** —— 这些是 Konsole 70k 行的主要去向，与"基础 VT"无关，一律不做；
- **wezterm 的 socketpair 双线程解析、自建 mux server、SSH** —— 超出范围；
- 两家的扩展协议（kitty 图形与键盘协议、Sixel、iTerm 图片、鼠标上报、`\e]52` 剪贴板、semantic prompt 标记）一律推迟。**注意**：Konsole 那个 CSI 大 switch 里有一大半 case 属于这类扩展，评估它的 4.9k 行时要先把这部分扣掉。
