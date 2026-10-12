# TrimUI Smart Pro 木棋 · 五子棋

面向掌机的人机五子棋，具有 WGSL 木纹、棋子阴影、落子动画、胜利连线、提示和原创合成背景音乐。显示、手柄和部署方式与中国象棋一致：Rust + wgpu 30.0.1 / Vulkan `VK_KHR_display` + egui 0.36.2 + CPAL 0.18.2，直接显示到 1280×720 内屏。

## 操作与玩法

按键以实体标签为准：A 在右，B 在下，X 在上，Y 在左。

| 按键 | 操作 |
| --- | --- |
| 方向键 / 左摇杆 | 移动光标；菜单中上下选项、左右调整 |
| A | 落子、确认或调整菜单选项 |
| B | 关闭菜单、清除操作提示 |
| X | 悔棋，撤回最近一次玩家落子及其后的电脑落子 |
| Y | 请求推荐落点，绿圈仅标记，不代替落子 |
| START | 开局设置中开始；对局中打开 / 关闭菜单 |
| SELECT + START | 退出并返回掌机菜单 |

15×15 棋盘，黑棋先手，连续至少五子获胜，允许长连，无禁手。棋盘定位点位于 D4、D12、H8、L4、L12，四角加中心共五个。蓝框是光标，棋子上的小点标记上一手。玩家执白时电脑先下中心，光标从中心右侧起步。

启动进入开局设置，默认玩家执黑、普通难度，音效 60%、音乐 20%，均开启。对局菜单提供继续、悔棋、提示、下一局执棋及难度、静音、音效与音乐开关及独立音量、重新开始、退出；音量每次调整 10%。执棋和难度在新局生效，声音设置立即生效。重新开始返回开局设置。棋局和设置仅保存在本次运行中。

电脑思考期间可移动光标、打开菜单和悔棋；提示计算期间仍可落子。落子、悔棋、重开会取消旧任务，后台结果还会检查请求编号及棋局版本。电脑先手的开局棋子不能单独撤回，对局结束后仍可悔棋。

AI 保留原项目预算：简单采用局面评分；普通最多 2 层 / 250 毫秒；困难最多 4 层 / 1 秒。所有难度优先检查立即获胜和必要防守，提示使用困难档。

## 构建与安装

需要 Rust、`aarch64-unknown-linux-gnu` target、Zig、cargo-zigbuild；ADB 部署需要 adb，输入事件测试还需要主机 Node.js。

在 workspace 根目录执行：

```sh
just check
just build gomoku
just package gomoku
just deploy gomoku
```

使用 `cargo zigbuild` 构建 `aarch64-unknown-linux-gnu.2.33`，默认缓存为 `~/.cache/cargo-zigbuild`，复用根目标 ALSA 库。运行时使用固件 Vulkan 和 ALSA/dmix。

- `artifacts/Ports/Gomoku/trimui-gomoku`：aarch64 可执行文件及应用目录。
- `artifacts/gomoku-trimui-smart-pro.tar.gz`：解压到 SD 卡根目录会生成 `Ports/Gomoku`。
- ADB 安装到 `/mnt/SDCARD/Ports/Gomoku`，通过 `.trimui-gomoku` marker 保护目录归属。

掌机应用列表中选择 **五子棋 / Gomoku**。列表缓存时退出后重新进入。应用运行日志为安装目录中的 `gomoku.log`，输入测试日志为同目录中的 `input-test.log`。

中文字体优先读取 `/usr/trimui/res/full.ttf`，回退到 `regular.ttf`；沿用 `CHINESE_CHESS_FONT` 环境变量以覆盖其他固件的字体路径。音频不可用时对局仍可进行。

## 验证

```sh
# workspace 格式、规则 / AI / 会话 / 手柄 / 音频测试及 Clippy
just check

# 仅五子棋
just gomoku::check

# 不需要窗口的 GPU 着色器及读回测试
cargo test --locked -p trimui-gomoku gpu_board_smoke -- --ignored

# 限时运行并取回截图
just run gomoku -- --seconds 15 --capture /tmp/gomoku.png

# 自动构建、安装、限时冒烟测试，再注入手柄事件
just input-test gomoku
```

输入测试直接向现有 `/dev/input/event3` 注入事件；测试期间不要操作实体手柄。覆盖执黑及执白开局、AI 回应、提示、悔棋、重开、静音、音效 / 音乐开关及音量、组合键退出，保存 `artifacts/gomoku/input-test.log`、`setup.png` 和 `game.png`，并对截图中的木质棋盘、五个定位点和黑白棋子做像素检查。

限时包装器会 STOP MainUI，并在退出、错误和信号路径 CONT 恢复；独立看门狗为 30 秒，结束宽限期为 5 秒。主机检查明确的结果行和关键日志，以兼容旧版 adbd 的退出码行为。

CLI 支持 `--seconds N`（0 表示不限时）、`--probe`、`--capture PNG`、`--capture-after N`（默认 2 秒）、`--controller PATH`（默认 `/dev/input/js0`）、帮助和版本。时长必须是有限非负数。默认日志过滤为 `warn,trimui_gomoku=info`，可用 `RUST_LOG` 覆盖。

2026-10-12 在当前连接的 TrimUI Smart Pro 上验证了中文设置与对局界面、木纹及黑白棋子、双方开局、AI、提示、悔棋、重开、声音设置、组合键退出和 MainUI 恢复；无效控制器路径的异常退出也会恢复 MainUI。稳定阶段约 59–60 FPS，音频为 48 kHz、双声道 F32。截图编码和落子动画重绘会短暂降低帧率。事件测试、截图像素检查、独立 GPU 测试、workspace 34 项普通测试及 Clippy 通过；实体按键手感和实际听感需在掌机上体验。

本次执行环境拒绝访问用户默认 zigbuild 缓存，验证时临时使用 `CARGO_ZIGBUILD_CACHE_DIR=/tmp/trimui-gomoku-zigbuild`。项目命令仍使用默认用户缓存，仓库中没有构建缓存目录。

## 实现

- `game` / `ai` / `session`：原版棋规、搜索和后台请求版本控制。
- `board` / `board.wgsl`：原版木质棋盘、棋子、动画与标记；使用显式顶点缓冲兼容 GE8300，缓存静止画面，变化时重绘。
- `app` / `ui`：掌机动作、开局设置、对局菜单和中文布局。
- `display` / `render` / `input`：象棋已验证的原生显示、PowerVR 兼容、截图和 joystick 输入。
- `audio`：ALSA 输出，保留无锁有界事件队列、固定合成声部和音量渐变；缓冲欠载由后端恢复，不会永久禁用音效。

主机编译产物、安装包、日志、截图与输入事件均放在被忽略的 `target/` 或 `artifacts/` 中。
