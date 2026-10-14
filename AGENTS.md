# TrimUI Smart Pro 开发约定

本仓库面向已研究过的 TrimUI Smart Pro 固件。设备运行 TinaLinux（Linux 4.9.191、aarch64、glibc 2.33），ADB 可用但它不是 Android。

## 构建

- 子项目依赖版本统一在根 `Cargo.toml` 的 `[workspace.dependencies]` 中维护；子项目使用 `anyhow = { workspace = true }` 这样的内联表继承，不使用 `anyhow.workspace = true`；额外的 `features` 写在同一内联表中。
- 目标使用 `aarch64-unknown-linux-gnu.2.33`，通过 `cargo zigbuild` 交叉编译；不要直接用主机 glibc 链接掌机程序。
- 根 `toolchain/pkgconfig/alsa.pc` 和 `toolchain/device-libs/libasound.so.2` 是构建 CPAL/ALSA 所需的最小目标库；运行时使用掌机固件自带库。
- 构建、打包、ADB 部署和实机测试由根 `justfile` 与各子项目的 `justfile` 管理。

## 显示与图形

- 内屏为 1280×720，GPU 是 PowerVR Rogue GE8300。当前固件没有可用的 X11/Wayland 会话，DRM 节点也没有可用的 CRTC/connector。
- 普通 winit Linux 窗口路径不能直接使用。显示层使用 wgpu 30.0.1 的 Vulkan `VK_KHR_display`，通过 `dc_sunxi` display/plane 建立 surface，并导入原生交换链图像。
- 不要把显示实现改回标准窗口 surface 或仅依赖 DRM；驱动需要现有的 Vulkan 信号量兼容处理。

## 应用图标

- 图标使用 256×256 的透明画布，主体四周必须保留 padding，并向上放置；下方留出连续透明空白，供 MainUI 显示应用名称。背景和阴影也必须限制在主体范围内，不能铺满画布或侵占下方留白。
- 默认参照中国象棋和五子棋图标：主体约 180×180、水平居中、顶部约 14 像素，底部约 60 像素透明留白。维护 `icon.svg` 源文件，并同步生成 8-bit RGBA 的 `icon.png`。

## 音频、输入和字体

- 音频走固件 ALSA/dmix；CPAL 0.18.2 的默认输出实测为 48 kHz、双声道、F32。不要假设有 PulseAudio。
- 控制器默认为 `/dev/input/js0`，对应 evdev `/dev/input/event3`。实体标签映射为 A=右、B=下、X=上、Y=左；SELECT + START 退出。
- 输入注入测试会直接写入现有 `event3`，测试期间不要操作实体手柄。
- 中文字体优先读取 `/usr/trimui/res/full.ttf`，再回退到 `regular.ttf`；其他固件可通过 `CHINESE_CHESS_FONT` 覆盖。

## 设备运行与部署

- MainUI 启动应用前会让出显示。限时测试脚本会 STOP MainUI，并必须在所有退出路径 CONT 回去；保留独立超时和结束宽限期。
- 设备上的应用安装在 `/mnt/SDCARD/Ports/<Name>`。部署前用项目 marker 文件确认目录归属，禁止覆盖无标记的目录。
- 旧版 adbd 不可靠地传播远端退出码；设备测试应检查脚本文本中的结果行和关键日志，而不是只依赖 `adb shell` 的退出码。
- 设备日志写在应用目录中，主机日志、截图、输入事件、安装包和其他临时内容放在被忽略的 `artifacts/` 下。
