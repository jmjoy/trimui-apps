# TrimUI Smart Pro 中国象棋

面向掌机的人机象棋，使用 Rust + wgpu/Vulkan + egui + CPAL，直接显示到 1280×720 内屏，不依赖 X11、Wayland 或 winit。

## 操作

按键以掌机实体标签为准：A 在右，B 在下，X 在上，Y 在左。

| 按键 | 操作 |
| --- | --- |
| 方向键 / 左摇杆 | 移动棋盘光标；菜单中上下选项、左右调整 |
| A | 选子、落子、确认菜单选项 |
| B | 取消选子、关闭对局菜单 |
| X | 悔棋，撤回你和电脑的一轮走子 |
| Y | 选子后跳到下一个合法落点 |
| START | 新对局界面直接开始；对局中打开 / 关闭菜单 |
| SELECT + START | 退出并返回掌机菜单 |

新对局可选择执红 / 执黑、简单 / 普通 / 困难、音效。红方始终在下，黑方在上。蓝框是光标，金圈是选中棋子，红点 / 红圈是合法落点。对局菜单提供继续、悔棋、调整难度和音效、重新开始、退出。电脑思考时可移动光标或打开菜单；悔棋需等电脑落子，重开会取消旧计算。

保留原项目的九宫、河界、蹩马腿、炮架、将帅照面、将死、困毙、三次重复和长将判负。AI 采用 alpha-beta 和逐层加深，三档最大深度分别为 2 / 3 / 4，思考预算为 0.35 / 1.2 / 3 秒；到时采用最后一轮完整搜索的结果。音效覆盖落子、吃子、将军和胜负，音频设备不可用时仍能对局。

## 构建和安装

需要 Rust `aarch64-unknown-linux-gnu` target、Zig、cargo-zigbuild；通过 USB 安装和测试还需要 adb。项目使用 glibc 2.33，复用 `toolchain/pkgconfig/alsa.pc` 和 `toolchain/device-libs` 中的目标 ALSA 库。

在 workspace 根目录执行：

```sh
just build chinese_chess
just deploy chinese_chess
# 另行部署并运行冒烟测试
just chinese_chess::smoke-test
```

根目录省略模块名时处理全部项目：`just package` 生成 `artifacts/trimui-apps-trimui-smart-pro.tar.gz`；`just deploy` 上传并安装该压缩包。部署仅安装，不启动应用；`just chinese_chess::smoke-test` 会先部署该模块，再运行冒烟测试。

构建结果：

- `artifacts/Ports/ChineseChess/trimui-chinese-chess`：aarch64 可执行文件。
- `artifacts/chinese-chess-trimui-smart-pro.tar.gz`：SD 卡安装包，解压到 SD 卡根目录会生成 `Ports/ChineseChess`。

ADB 安装位置为 `/mnt/SDCARD/Ports/ChineseChess`，在掌机“应用”中选择 **中国象棋 / Chinese Chess**。若当前应用列表已缓存，退出列表后重新进入。运行日志在应用目录的 `chess.log`。部署会保护没有项目 marker 的目录；卸载时删除该目录即可。

默认从 `/usr/trimui/res/full.ttf` 读取中文字体，回退到 `regular.ttf`；无需复制字体。其他固件可用环境变量 `CHINESE_CHESS_FONT` 指定 CJK TTF/TTC 文件。运行时使用固件自带 Vulkan 和 ALSA 库。

## 验证

```sh
# 本机：棋规、AI、取消旧计算、手柄走子与悔棋、方向键重复
just fmt
just test
just clippy

# 掌机限时运行，退出 / 报错 / 信号都会恢复 MainUI
just chinese_chess::run -- --seconds 15 --capture /tmp/chinese-chess.png

# 实机手柄事件测试：需额外安装 Node.js，只用于生成测试事件
just chinese_chess::input-test
```

`input-test` 向现有 `TRIMUI Player1` 的 `/dev/input/event3` 注入按键，依次验证执红走子、AI 回应、悔棋、重开、执黑走子及悔棋、组合键退出，保存 `artifacts/input-test.log` 和 `artifacts/game.png`。测试期间不要操作实体手柄。限时包装器有独立的 30 秒看门狗和 5 秒结束宽限期；从应用菜单启动则可持续对局。

命令行使用 clap 解析，支持 `--seconds N`（0 表示不限时）、`--capture PNG`、`--capture-after N`（默认 2 秒）、`--controller PATH`（默认 `/dev/input/js0`）、`--probe`（仅验证原生显示 surface，不提交画面）、`-h` / `--help` 和 `-V` / `--version`。时长必须是有限的非负数。截图读回和 PNG 编码会短暂降低帧率。

日志使用 log + env_logger，默认过滤规则为 `warn,trimui_chinese_chess=info`：显示应用信息日志及依赖库的警告和错误。可通过 `RUST_LOG` 调整，例如 `RUST_LOG=warn` 只显示警告和错误；掌机启动和测试脚本也会保留已设置的 `RUST_LOG`。

2026-10-11 在当前连接的 TrimUI Smart Pro 上验证：中文棋盘和菜单、Vulkan 连续呈现、48 kHz 双声道 F32 音频流、红黑双方手柄落子、AI 回应、悔棋、重开、退出和 MainUI 恢复。本机 11 项测试与 Clippy 通过；实机稳定阶段约 59–60 FPS。输入验证使用注入事件，实体按键手感及实际听感需在掌机上体验。

## 实现

- [game.rs](src/game.rs)：原项目棋规；搜索副本不再携带整局历史，棋子名称区分红黑。
- [ai.rs](src/ai.rs)：有时限、可取消的 alpha-beta；缓存区分精确值与上下界并限制容量。
- [app.rs](src/app.rs)：手柄操作、对局状态、异步 AI 通道及旧任务取消。
- [ui.rs](src/ui.rs)：适配 720p 的中文棋盘、光标、菜单和走子记录。
- [display.rs](src/display.rs)：复用 wgpu-demo 的 `VK_KHR_display` 交换链、图像导入和 PowerVR 信号量兼容处理。
- [render.rs](src/render.rs)：egui-wgpu 渲染、中文字体和截图读回。
- [input.rs](src/input.rs)：Linux joystick API、任天堂实体标签映射、死区及方向键重复。
- [audio.rs](src/audio.rs)：原项目合成音效，使用 CPAL 0.18.2 的默认 48 kHz 输出和固件 ALSA / dmix 链路；缓冲欠载由后端恢复。

编译产物、安装包、日志和截图保存在 workspace 根目录的 `artifacts/`，已忽略。
