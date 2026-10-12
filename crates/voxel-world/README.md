# TrimUI Smart Pro 方块世界

自制 Minecraft 风格的第一人称 3D 创造沙盒。使用 Rust 2024、wgpu 30.0.1 / Vulkan `VK_KHR_display`、egui 0.36.2 和 CPAL 0.18.2，直接显示到掌机内屏。

256×256×64 的有限世界按种子生成丘陵、树林与沙地。可步行、跳跃、飞行、挖掘和建造；草、泥土、石头、沙、原木、树叶、木板和砖块无限供应。底层基岩不可破坏，世界边界限制移动与建造，操作距离为 6 格。首版为创造模式，没有生存、合成、怪物、流体或联机。

## 操作

以掌机实体标签为准：A 在右，B 在下，X 在上，Y 在左。

| 按键 | 操作 |
| --- | --- |
| 左摇杆 | 移动 |
| 右摇杆 | 视角 |
| R2 / L2 | 挖掘 / 放置，按住每 180 毫秒操作一次 |
| L / R | 前一个 / 后一个快捷栏方块 |
| B | 跳跃；飞行时上升 |
| A | 飞行时下降；菜单确认 |
| X | 切换步行 / 飞行 |
| Y | 方块库 |
| START | 暂停 / 继续；启动界面确认当前选项 |
| SELECT + START | 保存并退出，返回 MainUI |
| 方向键 / 左摇杆 | 菜单选择；左右调整设置 |

暂停菜单可立即保存、调整视距（2–4 个区块）、视角灵敏度（0.5–2.0）、音效音量、重建世界或保存退出。暂停与方块库冻结模拟；重建需要确认，默认选中保留当前世界。

默认视距为 3 区块、灵敏度为 1.0、音效为 60%、步行模式、草方块。摇杆采用 18% 死区，L2/R2 通过轴阈值及迟滞识别；Linux joystick 的轴与按钮映射通过只读 ioctl 查询。

## 存档

应用目录中的 `save/world.dat` 保存单个世界，包括全部方块、地形种子、位置、视角、飞行状态、选中方块和设置。每 30 秒、暂停及正常退出（含 TERM 信号与限时运行）保存一次。后台线程压缩并写入同目录临时文件，同步完成后原子替换；部署更新程序时保留该目录。

读取失败时显示提示并保留原文件，禁止自动覆盖；需要选择新建世界并确认后才会替换损坏的存档。写入失败会显示提示，原存档仍可使用。强制断电或 KILL 最多丢失最近一次成功自动保存之后的修改。

## 构建和部署

需要 Rust `aarch64-unknown-linux-gnu` target、Zig、cargo-zigbuild、Just；ADB 部署需要 adb，输入测试需要 Node.js。复用根目标 ALSA 库，通过 glibc 2.33 交叉编译：

```sh
just check
just build voxel_world
just package voxel_world
just deploy voxel_world
just input-test voxel_world
```

- 安装目录：`/mnt/SDCARD/Ports/VoxelWorld`，marker 为 `.trimui-voxel-world`。
- 安装包：`artifacts/voxel-world-trimui-smart-pro.tar.gz`，解压到 SD 卡根目录。
- 在掌机应用列表中选择 **方块世界 / Voxel World**；运行日志为应用目录的 `voxel-world.log`。
- 主机日志、截图、输入事件与测试辅助程序放在 `artifacts/voxel-world/`。

中文字体优先读取 `/usr/trimui/res/full.ttf`，回退到 `regular.ttf`；沿用 `CHINESE_CHESS_FONT` 覆盖。音频使用固件 ALSA/dmix；音频不可用时仍可游戏。

## 验证与命令行

```sh
just voxel_world::check
cargo test --locked -p trimui-voxel-world gpu_scene_smoke -- --ignored
just run voxel_world -- --seconds 15 --capture /tmp/voxel-world.png
just input-test voxel_world
```

CLI 支持 `--seconds N`（0 不限时）、`--probe`、`--capture PNG`、`--capture-after N`（默认 2 秒）、`--controller PATH`（默认 `/dev/input/js0`）、`--save-dir DIR`（默认 `save`）、`--seed U64`、帮助和版本。种子仅在没有既有存档时使用。时长必须有限且非负。`RUST_LOG` 默认 `warn,trimui_voxel_world=info`。截图读回和 PNG 编码会短暂降低帧率，`just run` 会将指定截图取回为 `artifacts/voxel-world/capture.png`。

部署冒烟和输入测试使用 `/tmp/` 中的独立存档，不会改动玩家存档。输入测试直接向现有 `/dev/input/event3` 写事件，持续注入轴值以避免固件采样覆盖测试输入；测试期间不要操作实体手柄；验证挖放、跳跃、飞行、双摇杆、快捷栏、方块库、设置、组合键退出和存档重载，并取回 `setup.png`、`game.png`、日志。

限时测试保留独立的 30 秒看门狗与 5 秒结束宽限期；所有退出路径恢复 MainUI。主机检查明确结果行和关键日志，以兼容旧版 adbd 的退出码行为。

## 实现

世界使用完整驻留的 4 MiB 方块数组与 16³ 区块。网格仅输出暴露面，合并同材质面，在后台线程构建；区块版本和世界编号拒绝过期结果，修改边界方块会重建邻区块。GPU 网格只缓存玩家附近区域，绘制前执行视锥剔除。

纹理由程序生成，使用纹理数组与重复采样保持合并面上的像素纹理尺寸；显式顶点与索引缓冲、深度测试、背面剔除和距离雾适配 GE8300。场景以 960×540 渲染并放大，HUD 和中文菜单使用原生 1280×720。模拟为 60 Hz 固定步长，帧率降低时限制补帧数量。

## 实机验证记录

2026-10-12 在当前 TrimUI Smart Pro 上通过原生 Vulkan 连续呈现、中文菜单、双摇杆、模拟扳机、挖放、快捷栏、跳跃、飞行、方块库、视距设置、组合键退出与存档重载测试。生成世界与修改后存档的摘要不同，重载摘要相同；直接读取存档确认位置、视角、飞行、视距 4、所选泥土及音量均恢复。稳态约 58–60 FPS；截图编码会短暂降低帧率。

workspace 47 项常规测试、格式检查及 Clippy 通过；独立 GPU 像素测试通过。错误、TERM 信号及 30 秒看门狗退出后均恢复 MainUI；该固件 BusyBox 的 timeout 可返回子程序自己的退出码，因此看门狗验证同时检查实际时长、保存日志和 MainUI 状态。安装二进制与主机打包文件的 SHA-256 一致，最高 glibc 版本需求为 2.33。

实机操作验证使用事件注入，音频验证确认 48 kHz、双声道 F32 流正常建立；实体按键手感与实际听感需在掌机上体验。
