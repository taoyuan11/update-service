# Update Service

面向桌面和移动应用的自托管更新服务器。后端使用 Rust/Axum/PostgreSQL，管理后台使用 Vue 3/TypeScript。

## 快速启动

```bash
cp .env.example .env
# 编辑 .env，至少替换 INITIAL_ADMIN_PASSWORD 和 SETTINGS_MASTER_KEY
openssl rand -base64 32
docker compose up --build
```

打开 `http://localhost:8088` 登录管理后台。首次启动会使用 `.env` 中的管理员账号创建初始管理员。

## 公共更新接口

```text
GET /api/public/apps/{app_id}/update?current_version=1.2.0&channel=stable&platform=windows-x64
```

响应为 `200` 时包含最新安装包的 `download_url`、`sha256`、更新日志和元数据；不存在更新时返回 `204`。应用和版本由管理后台创建，应用 ID 不可变。

## 本地开发

```bash
docker compose up postgres minio -d
cd backend && cargo run
cd frontend && npm install && npm run dev
```

后端默认监听 `8080`，`cargo run` 会自动读取项目根目录的 `.env`，Vite 将 `/api` 代理给该端口。MinIO 为可选的 S3 兼容存储联调服务，控制台位于 `http://localhost:9001`。

## 存储配置

管理员在“存储设置”创建本地磁盘或 S3 配置，并激活一个配置后方可上传。S3 Secret Key 使用 `SETTINGS_MASTER_KEY` 派生的 AES-256-GCM 密钥加密后保存；接口不会返回明文。切换活动配置只影响新上传的文件，历史文件仍使用其原有配置读取。

### 存储迁移

管理员可以在“存储设置”发起 S3 到 S3、S3 到本地存储或本地存储到 S3 的后台迁移。任务启动时目标配置会成为新的上传目标；系统逐个复制数据库中登记的安装包，校验大小和 SHA-256 后更新文件引用。源端文件始终保留，bucket 或目录中未被数据库引用的对象不会迁移。

迁移任务支持查看进度、取消以及重试失败项。单个对象失败后会指数退避并最多自动尝试 5 次，任务历史保留 30 天。服务重启后会自动恢复未完成任务。

## 生产注意事项

- 设置 `COOKIE_SECURE=true` 并经 HTTPS 反向代理访问。
- `SETTINGS_MASTER_KEY` 一旦更换将无法解密已有 S3 凭据，请使用密钥管理系统妥善保管。
- 本地存储目录必须是 API 容器可读写的持久卷。
- `UPLOAD_TEMP_DIR` 同时用于上传和跨存储迁移。S3 到 S3 也会经服务临时目录中转，因此该目录至少需要容纳最大的单个安装包。
- 迁移期间源文件不会自动清理；确认不再需要回滚后，请在源存储侧自行安排生命周期或清理策略。
