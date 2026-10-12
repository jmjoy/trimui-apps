# TrimUI 交叉编译依赖

`alsa.pc` 和 `libasound.so.2` 与当前 TrimUI Smart Pro 固件兼容，仅用于在主机上为 CPAL/ALSA 链接出兼容 glibc 2.33 的 aarch64 程序。

掌机运行时使用自身的 `/usr/lib` ALSA 库；Vulkan、EGL 和其他图形库也由固件提供，不应复制进应用包。
