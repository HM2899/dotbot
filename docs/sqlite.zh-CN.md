# SQLite 数据保存

`dotbot serve` 默认将团队数据保存到 `<home>/dotbot.sqlite3`，无需额外安装数据库服务。使用内置 SQLite，适用于 x86_64 和 ARM64 的原生编译。

保存范围：Bot、群聊、偏好和个人资料、模型/社交平台/连接配置、密钥、定时任务、执行与恢复记录、阅读位置、技能专长配置，以及完整聊天和附件元数据。团队状态与引擎会话信息在同一个事务提交；聊天按消息 ID 更新，重复工具结果不会产生重复消息。

附件本体、Markdown 记忆、技能包及历史资源、桌面文件和浏览器配置仍保留在原目录，因为工具及外部应用需要直接访问它们。管理员密码哈希和网页登录会话继续由 `web-auth` 私密文件保存；旧本地启动令牌已停用，不再读取或生成。数据库不是整个团队文件夹的替代品。

## 已有团队迁移

停止旧服务、备份完整团队目录后，使用同一个 home 启动新版本：

```sh
./target/release/dotbot --home ~/.teambot serve
```

首次启动在一个事务中导入团队根目录 JSON 快照和 `chats/*.jsonl`。源文件保留，之后以 SQLite 为准，不再双写这些旧快照。配置或聊天存在无效 JSON 时，迁移失败且事务回滚，原文件不修改；修复文件后重新启动可继续。成功迁移不会在以后启动时重新导入，因此修改旧 JSON 不会覆盖数据库中的新数据。

新建团队也默认直接写 SQLite。需要继续使用原文件模式时，在尚未迁移的团队使用：

```sh
./target/release/dotbot --storage json --home /path/to/legacy-team serve
```

已有 `dotbot.sqlite3` 时拒绝切回 JSON，防止读取过期快照。管理员密码设置、技能包安装、DS Bot 导入命令仍可使用；导入不会覆盖已有的 SQLite 团队。

## 备份

数据库使用 WAL、完整同步和 5 秒写锁等待；文件权限为 0600。不要在运行中只复制 `.sqlite3` 而遗漏未检查点的 WAL。最直接的方式是停止服务后备份**完整团队目录**，包括可能存在的 `.sqlite3-wal` / `.sqlite3-shm`、`web-auth`、media、memory、plugin、desktops 等。

在线数据库备份可使用 Python 标准库的 SQLite 备份接口，再另外备份附件与其余目录：

```python
import sqlite3
from pathlib import Path
import os
source = sqlite3.connect(str(Path.home() / '.teambot' / 'dotbot.sqlite3'))
backup_path = '/private/backup/dotbot.sqlite3'
fd = os.open(backup_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
os.close(fd)
with sqlite3.connect(backup_path) as target:
    source.backup(target)
source.close()
```

恢复时先停止服务，恢复数据库与匹配的附件和目录，避免把旧 WAL 文件混入另一份数据库。备份包含密钥及登录数据，应只对管理员可读。

## 验证

```sh
cargo test
node dev/sqlite.e2e.mjs
```

测试覆盖失败迁移回滚、源文件保留、聊天更新顺序、并发写入、配置与聊天重启恢复、Bot 模型不变、数据库完整性和过期 JSON 防回退。
