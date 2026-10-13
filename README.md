# TrimUI Apps

这是一个用于 TrimUI 掌机自制游戏和应用的 Rust Cargo workspace，子项目统一放在 `crates/` 目录。

## 项目

| 项目 | Just module | 安装目录 |
| --- | --- | --- |
| [中国象棋](crates/chinese-chess/README.md) | `chinese_chess` | `/mnt/SDCARD/Ports/ChineseChess` |
| [木棋 · 五子棋](crates/gomoku/README.md) | `gomoku` | `/mnt/SDCARD/Ports/Gomoku` |
| [方块世界](crates/voxel-world/README.md) | `voxel_world` | `/mnt/SDCARD/Ports/VoxelWorld` |

各子项目维护自己的 `justfile`，根 `justfile` 提供统一入口：

```sh
just fmt
just test
just clippy
just build chinese_chess
just deploy chinese_chess
just build gomoku
just deploy gomoku
just input-test gomoku
just deploy voxel_world
just input-test voxel_world
```

也可以直接调用模块命令：

```sh
just chinese_chess::build
just chinese_chess::run
just gomoku::build
just gomoku::run
```

新增项目时，将项目放在 `crates/` 下，在根 `justfile` 注册一个 Just module，并在项目目录维护自己的 `justfile`。

## 许可证

本项目采用 [木兰宽松许可证，第 2 版（Mulan PSL v2）](LICENSE)。
