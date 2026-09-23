# zlm-node

**zlm-node** 是一个用于将 ZLMediaKit 节点状态上报到管理平台（如 GBHub）的轻量级代理程序。它自动获取公网 IP、更新 ZLM 的 `rtc.externIP`、周期性上报节点信息，并可选地接管录像 hook、上传录像到 S3 对象存储。

---

## ✨ 功能特性

- ✅ 自动获取公网 IP（支持多种来源，支持自定义 IP 服务）
- ✅ 周期性上报节点状态到管理平台（`MGR_URL`）
- ✅ 自动更新 ZLM 的 `rtc.externIP`（可选）
- ✅ 支持手动指定 IP 或网卡 IP
- ✅ 支持自定义 IP 获取服务地址（`IP_ECHO_API`）
- ✅ 内置公共 IP 查询 API 作为后备，确保高可用
- ✅ 支持 TLS 证书校验开关（自签名场景）
- ✅ 低资源占用，适合嵌入式或家庭设备部署
- ✅ **启动时自动从 ZLM 拉取 `mediaServerId` 作为节点标识**
- ✅ **录像 hook 服务：接收 ZLM 的 `on_record_mp4` 事件，上传 S3 并通知 mgr**
- ✅ **流式上传大文件（内存恒定 ~64KB，不受录像大小影响）**
- ✅ **hook 自动重设（ZLM 重启后自动恢复）**

---

## 📦 快速开始

### cargo 安装

```bash
cargo install zlm-node
```

### 编译

```bash
cargo build --release
```

编译完成后，可执行文件位于 `target/release/zlm-node`。

### 运行

```bash
./target/release/zlm-node
```

默认配置会尝试连接 `http://127.0.0.1:9080` 的 ZLM 服务，并向 `http://127.0.0.1:3002/api/zlm/report-status` 上报状态。请通过环境变量覆盖默认值。

---

## ⚙️ 环境变量配置

所有配置均通过环境变量完成，无需配置文件。

### 基础配置

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| NODE_TOKEN | 空 | 节点鉴权令牌（上报时通过 `X-Node-Token` 头发送 blake3 哈希） |
| SERVER_ID | 1 | 节点 ID 兜底值（启动时会被 ZLM 的 `mediaServerId` 自动覆盖） |
| API_BASE | http://127.0.0.1:9080 | ZLM API 基础地址 |
| SECRET | 935v73f7-bb6b-4889-a715-d9eb2d1936aa | ZLM API 密钥 |
| MGR_URL | http://127.0.0.1:3002/api/zlm/report-status | 管理端状态上报接口 |
| CUSTOM_IP | 无 | 手动指定公网 IP（优先级最高） |
| INTERFACE | 无 | 指定网卡名称，从该网卡获取 IPv4 地址 |
| IP_ECHO_API | 无 | 自定义 IP 获取服务地址，优先级高于内置公共 API |
| ZLM_HTTP_PORT | 9080 | ZLM HTTP 端口，用于拼接 `http_fmp4_base` |
| USE_HTTPS | false | 是否使用 HTTPS 协议拼接 `http_fmp4_base` |
| STATIC_BASE | 无 | 静态资源基地址，若不设置则与 `http_fmp4_base` 相同 |
| HTTP_FMP4_BASE | 无 | HTTP-FMP4 播放基地址，支持占位符；不设置则自动生成 |
| REPORT_INTERVAL_SECS | 30 | 上报间隔（秒） |
| IP_REFRESH_INTERVAL_SECS | 5 | IP 刷新间隔（秒） |
| TLS_ACCEPT_INVALID_CERTS | false | 是否接受无效 TLS 证书（自签名场景） |
| ENABLE_RTC_EXTERN_IP_UPDATE | true | 是否自动更新 ZLM 的 `rtc.externIP` |

> **IP 获取优先级**：
> `CUSTOM_IP` > `INTERFACE` > `IP_ECHO_API` > 内置公共 API（`ip.3322.net`、`ip.automate.org.cn`）

### 录像 hook 服务（可选）

启用后，zlm-node 会启动一个本地 HTTP 服务，接管 ZLM 的 `on_record_mp4` 事件，自动将录像上传到 S3 并通知 mgr 写数据库。

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| RECORD_HOOK_ENABLE | false | 是否启用录像 hook 服务 |
| HOOK_LISTEN | 127.0.0.1:3004 | hook 服务监听地址（与 ZLM 同机，用 loopback 即可） |
| MGR_BASE | http://127.0.0.1:3002 | mgr 的 base URL（不含路径），用于调用内部接口 |
| INTERNAL_API_TOKEN | 空 | mgr 的内部通信令牌，**启用录像 hook 时必填** |
| RECORD_S3_ENABLE | false | 是否上传录像到 S3（关闭时只通知 mgr，本地保留） |
| RECORD_KEEP_ON_FAILURE | true | S3 上传失败时是否保留本地文件 |
| RECORD_HOOK_SYNC_INTERVAL_SECS | 60 | 定期兜底重设 ZLM hook 的间隔（秒） |

---

## 占位符支持

在 `HTTP_FMP4_BASE` 和 `STATIC_BASE` 中可使用以下占位符，程序会在每次上报时替换为实际 IP：

- `{ip}`：替换为当前上报所用的公网 IP（即 `public_ip`）。
- `{interface:网卡名}`：替换为指定网卡的第一个 IPv4 地址，例如 `{interface:eth1}`。

**示例**：

```bash
# 播放地址使用 eth1 网卡的 IP，而上报 IP 使用公网 IP
export HTTP_FMP4_BASE="http://{interface:eth1}:9080/"

# 播放地址使用公网 IP，但指定 HTTPS 和自定义路径
export HTTP_FMP4_BASE="https://{ip}:9443/live/"
```

---

## 🎥 录像 hook 服务

启用 `RECORD_HOOK_ENABLE=true` 后，zlm-node 会：

1. **启动时**从 ZLM 拉取 `mediaServerId`，用它作为节点标识（避免手工配 `SERVER_ID` 出错）
2. **启动本地 HTTP 服务**（默认 `127.0.0.1:3004`），接收 ZLM 的 `on_record_mp4` hook
3. **自动重设 ZLM 的 hook 地址**，让 ZLM 录完直接投递到 zlm-node
4. **每 60 秒兜底重设**（防止 ZLM 重启后配置丢失）
5. 收到 hook 后：
   - 立即返回 `200`（不阻塞 ZLM）
   - 后台：调 mgr 拿 presigned URL → 流式上传 S3 → 通知 mgr 落库
   - **S3 关闭时**：跳过上传，本地保留，仅通知 mgr

### 数据流

```
ZLM 录完 → hook → zlm-node (127.0.0.1:3004)
  ↓
zlm-node 立即返回 200
  ↓
后台：
  ① 调 mgr /api/internal/presign-record 拿 S3 presigned URL
  ② 流式 PUT 文件到 S3
  ③ 通知 mgr /internal/on-record-event（带 S3 URL 或本地相对路径）
  ↓
mgr 写 DB + 处理客户回调
```

### 启用示例

```bash
export RECORD_HOOK_ENABLE=true
export HOOK_LISTEN=127.0.0.1:3004
export MGR_BASE=http://mgr.internal:3002
export INTERNAL_API_TOKEN=q123456778
export RECORD_S3_ENABLE=true
export RECORD_KEEP_ON_FAILURE=true
./zlm-node
```

启动后日志：

```
[init] detected ZLM mediaServerId = zlmediakit-abc123
[record-hook] listening on 127.0.0.1:3004
[record-hook] set ZLM hook.on_record_mp4 = http://127.0.0.1:3004/hook/on_record_mp4
```

### 本地存储模式（关闭 S3）

```bash
export RECORD_S3_ENABLE=false
```

此时 zlm-node **不上传 S3，只通知 mgr**。mgr 侧用 `relative_url` + 该节点的 `static_base` 拼出本地播放 URL，录像文件保留在 ZLM 节点的本地磁盘。

**⚠️ 注意**：多节点部署下，本地文件只有 ZLM 节点自己能访问——前端回放需要 mgr 能路由到对应 ZLM 节点。

### 与普通 hook 的关系

**只有 `on_record_mp4` 走 zlm-node**，其它 hook（`on_play` / `on_publish` / `on_stream_changed` 等）保持原有配置不变。

- ZLM 普通 hook → sip（直连或经 tunnel）
- ZLM 录像 hook → zlm-node（本机 loopback）

**两条独立路径，互不干扰。**

---

## TLS 隧道功能（可选）

`zlm-node` 支持将 ZLMediaKit 的 Hook 请求通过 TLS 加密隧道转发到远程管理平台。启用后，ZLM 的 Hook 地址只需指向本地隧道端口，由 `zlm-node` 负责加密传输和网络穿透。

> **注意**：录像 hook（`on_record_mp4`）**不走 TLS 隧道**——它由 zlm-node 直接处理并通知 mgr。

### 配置环境变量

| 环境变量 | 类型 | 默认值 | 说明 |
|----------|------|--------|------|
| TUNNEL_ENABLE | bool | false | 是否启用 TLS 隧道 |
| TUNNEL_LOCAL_ADDR | string | 127.0.0.1:18080 | 本地隧道监听地址 |
| TUNNEL_REMOTE_ADDR | string | 空 | 远程 TLS 服务地址（必须设置） |
| TUNNEL_SERVER_NAME | string | tunnel.example.com | TLS 服务器名称（域名） |
| TUNNEL_CA_CERT | string (可选) | 无 | 自定义 CA 证书路径 |
| TUNNEL_CLIENT_CERT | string (可选) | 无 | 客户端证书路径（mTLS） |
| TUNNEL_CLIENT_KEY | string (可选) | 无 | 客户端私钥路径（mTLS） |
| TUNNEL_MAX_CONNECTIONS | usize | 100 | 最大并发连接数 |
| TUNNEL_IDLE_TIMEOUT_SECS | u64 | 300 | 连接空闲超时（秒），0 表示禁用 |
| TUNNEL_CONNECT_TIMEOUT_SECS | u64 | 10 | 远程连接超时（秒） |
| TUNNEL_RETRY_DELAY_SECS | u64 | 2 | 连接失败后初始重试延迟（秒） |
| TUNNEL_MAX_RETRY_DELAY_SECS | u64 | 30 | 指数退避最大延迟（秒） |
| TUNNEL_BUFFER_SIZE | usize | 65536 | 数据传输缓冲区大小（字节） |
| TUNNEL_DISABLE_TLS_RESUMPTION | bool | false | 是否禁用 TLS 会话恢复 |
| TUNNEL_AUTH_TOKEN | 空 | 否 | 简单密码认证 token，客户端与服务端一致；为空则跳过认证。 |

### 启用隧道

设置环境变量并启动 `zlm-node`：

```bash
export TUNNEL_ENABLE=true
export TUNNEL_LOCAL_ADDR=127.0.0.1:18080
export TUNNEL_REMOTE_ADDR=tunnel.example.com:443
export TUNNEL_SERVER_NAME=tunnel.example.com
export TUNNEL_CA_CERT=/etc/zlm-node/ca.pem
export TUNNEL_AUTH_TOKEN=my-secret-token
./zlm-node
```

### 修改 ZLMediaKit 配置

编辑 ZLMediaKit 的 `config.ini`，将 `[hook]` 部分的**非录像 hook** 改为本地隧道地址：

```ini
[hook]
on_play=http://127.0.0.1:18080/hook/on_play
on_publish=http://127.0.0.1:18080/hook/on_publish
on_stream_changed=http://127.0.0.1:18080/hook/on_stream_changed
on_rtsp_realm=http://127.0.0.1:18080/hook/on_rtsp_realm
# 其他 hook 同样修改
# 注意：on_record_mp4 保持原有配置（由 zlm-node 自动改）
```

重启 ZLMediaKit 使配置生效。之后所有非录像 Hook 请求都会通过 `zlm-node` 加密转发到远程服务。

---

## 🔧 配置示例

### 家庭动态公网 IP 场景（推荐使用自建 IP 服务）

假设您已在云服务器部署了 IP 身份服务（返回纯 IP 文本），地址为 `http://your-cloud-server:8080`。

```bash
export NODE_TOKEN=your_secret_token_here
export SERVER_ID=home-node-01
export API_BASE=http://127.0.0.1:9080
export SECRET=your_zlm_secret
export MGR_URL=http://your-gbhub-domain/api/zlm/report-status
export IP_ECHO_API=http://your-cloud-server:8080
export REPORT_INTERVAL_SECS=30
export IP_REFRESH_INTERVAL_SECS=5
export ENABLE_RTC_EXTERN_IP_UPDATE=true
./target/release/zlm-node
```

### 完整部署（状态上报 + 录像 hook + S3 上传）

```bash
# 状态上报
export API_BASE=http://127.0.0.1:9080
export SECRET=your_zlm_secret
export MGR_URL=http://mgr.internal:3002/api/zlm/report-status
export NODE_TOKEN=your_node_token

# 录像 hook
export RECORD_HOOK_ENABLE=true
export HOOK_LISTEN=127.0.0.1:3004
export MGR_BASE=http://mgr.internal:3002
export INTERNAL_API_TOKEN=q123456778
export RECORD_S3_ENABLE=true
export RECORD_KEEP_ON_FAILURE=true

./target/release/zlm-node
```

### 手动指定 IP（测试或固定 IP 场景）

```bash
export CUSTOM_IP=203.0.113.5
# 其他必需变量...
./target/release/zlm-node
```

### 使用网卡 IP（多网卡环境）

```bash
export INTERFACE=eth0
# 其他变量...
./target/release/zlm-node
```

---

## 🧪 验证

1. 确认 zlm-node 日志输出类似：

```
[init] detected ZLM mediaServerId = zlmediakit-abc123
Public IP: 123.45.67.89, Version: master(abc123)
Periodic report sent.
```

2. 检查管理端是否收到节点状态更新。

3. 如果启用了 `ENABLE_RTC_EXTERN_IP_UPDATE`，登录 ZLM 管理界面查看 `rtc.externIP` 是否已同步为公网 IP。

4. **如果启用了录像 hook**，看到以下日志说明就绪：

```
[record-hook] listening on 127.0.0.1:3004
[record-hook] set ZLM hook.on_record_mp4 = http://127.0.0.1:3004/hook/on_record_mp4
```

5. **手工触发录像 hook 测试**：

```bash
echo "test" > /tmp/test.mp4
curl -X POST http://127.0.0.1:3004/hook/on_record_mp4 \
  -H "Content-Type: application/json" \
  -d '{
    "file_path": "/tmp/test.mp4",
    "url": "record/test/test.mp4",
    "stream": "34020000001320000001_34020000001310000001",
    "app": "rtp",
    "vhost": "__defaultVhost__"
  }'
```

预期：`{"code":0}`，日志显示 S3 上传成功或本地保留。

---

## 🛠️ 常见问题

### 1. 为什么获取到的 IP 不正确？

- 如果使用了 `CUSTOM_IP`，请检查设置的值。
- 如果使用了 `INTERFACE`，确保该网卡存在且配置了 IPv4。
- 如果使用了 `IP_ECHO_API`，确保该服务可从 zlm-node 所在网络访问，且返回纯 IP 文本（可包含换行）。
- 若以上都未设置，则使用内置公共 API，可能受网络环境影响。

### 2. 如何彻底禁用内置公共 API？

将 `IP_ECHO_API` 始终设置为您的自建服务，并在网络层面阻止对 `ip.3322.net` 等域名的访问。代码层面保留后备逻辑不影响正常使用。

### 3. 自建 IP 服务如何实现？

任何能返回客户端公网 IP 的 HTTP 服务均可，例如使用 Nginx 的 `return 200 "$remote_addr";`，或运行一个极简的 Rust/Python 程序。

### 4. 上报间隔调多少合适？

家庭宽带 IP 变化不频繁，默认 `30` 秒已足够。如需更快感知 IP 变化，可适当降低至 `10` 秒。

### 5. 支持 Docker 部署吗？

完全支持。建议将可执行文件放入容器，并通过 `-e` 参数传入环境变量。

### 6. 录像 hook 启用后，本地文件怎么清理？

- **S3 模式（`RECORD_S3_ENABLE=true`）**：上传成功且 mgr 通知成功后才删本地
- **本地模式（`RECORD_S3_ENABLE=false`）**：文件保留在 ZLM 节点，由 ZLM 自身的录像保留策略清理

### 7. `mediaServerId` 和 `SERVER_ID` 是什么关系？

- zlm-node 启动时**自动从 ZLM 拉 `mediaServerId`**，用它作为节点标识上报给 mgr
- 手工配的 `SERVER_ID` 只作为拉取失败时的兜底
- **好处**：避免手工配置出错；ZLM 重装后自动跟随新 ID

### 8. 为什么 `RECORD_HOOK_ENABLE=true` 时必须配 `INTERNAL_API_TOKEN`？

录像 hook 需要调用 mgr 的两个内部接口（`presign-record` 和 `on-record-event`），这两个接口用 `X-Internal-Token` 鉴权，token 必须与 mgr 的 `INTERNAL_API_TOKEN` 一致。

### 9. S3 上传失败会丢录像吗？

不会——只要 `RECORD_KEEP_ON_FAILURE=true`（默认），S3 上传失败时本地文件会保留。可以：
- 排查 S3 / 网络问题
- 手动或脚本重传
- 临时关闭 S3（`RECORD_S3_ENABLE=false`）让 zlm-node 走本地模式

---

## 📄 许可证

本项目采用 [MIT License](LICENSE)。
