# TrimUI Apps

这是一个用于 TrimUI 掌机自制游戏和应用的 Rust Cargo workspace，子项目统一放在 `crates/` 目录。

## 项目

| 项目 | Just module | 安装目录 |
| --- | --- | --- |
| [中国象棋](crates/chinese-chess/README.md) | `chinese_chess` | `/mnt/SDCARD/Ports/ChineseChess` |
| [五子棋](crates/gomoku/README.md) | `gomoku` | `/mnt/SDCARD/Ports/Gomoku` |
| [方块世界](crates/voxel-world/README.md) | `voxel_world` | `/mnt/SDCARD/Ports/VoxelWorld` |

根 `justfile` 的通用命令默认处理全部子项目；`fmt` 只检查格式：

```sh
just fmt
just test
just clippy
just check
just build
just package
just deploy
just clean
```

`just package` 生成一个 `artifacts/trimui-apps-trimui-smart-pro.tar.gz`，包含所有应用的 `Ports/<Name>` 目录，可解压到 SD 卡根目录。`just deploy` 先打包，再通过 ADB 上传压缩包并安装，不启动应用或运行冒烟测试。安装前检查全部目标目录的项目 marker，更新保留存档、设置和日志。

通用命令可以指定模块；构建、打包和部署默认使用 `release`，也支持 `debug`：

```sh
just build gomoku
just package chinese_chess
just deploy voxel_world
just check gomoku
just build all debug
just package gomoku debug
```

运行、冒烟、输入和基准测试通过模块调用：

```sh
just chinese_chess::run -- --seconds 15 --capture /tmp/chinese-chess.png
just gomoku::smoke-test
just gomoku::input-test
just voxel_world::exit-test
just voxel_world::bench 40
```

`smoke-test` 自动部署对应模块后运行冒烟测试；`input-test` 在此基础上注入手柄事件。`run` 直接运行已安装的应用。也可进入子项目目录执行 `just build`、`just smoke-test` 等命令。主机侧流程回归检查使用 `node scripts/test-workflows.cjs`，通过 Cargo／ADB 替身运行，不访问设备。

公共主机脚本放在根 `scripts/`，项目专属脚本放在各项目的 `scripts/`。新增项目时，在根 `justfile` 注册模块，在 `scripts/common.sh` 登记元数据和全量项目列表，并维护项目的 justfile 与脚本。

## 许可证

本项目采用 [木兰宽松许可证，第 2 版（Mulan PSL v2）](LICENSE)。
