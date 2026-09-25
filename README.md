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
- ✅ **启动时自动从 ZLM 拉取 `mediaServerId` 作为节点标识，拿到后永久缓存**
- ✅ **每次上报前实测 ZLM 版本；ZLM 挂了就上报 `version="offline"`，上级据此判断 ZLM 离线**
- ✅ **内置 `--gen-token` 子命令，一键生成上报明文与哈希，安装脚本不再依赖 `b3sum`**
- ✅ **支持 `NODE_TOKEN_SALT` 静态盐：使用 keyed blake3，`b3sum` 命令无法直接算出正确 hash**
- ✅ **`NODE_TOKEN` 的 hash 只在进程启动时计算一次，之后零开销复用**
- ✅ **录像 hook 服务：接收 ZLM 的 `on_record_mp4` 事件，上传 S3 并通知 mgr**
- ✅ **流式上传大文件（内存恒定 ~64KB，不受录像大小影响）**
- ✅ **hook 自动重设（ZLM 重启后自动恢复），设成功判断 `code == 0`**
- ✅ **hook 请求体自带 `mediaServerId` 优先，本地缓存兜底**
- ✅ **只用一个 `NODE_TOKEN` 即可调用 mgr 全部接口（含录像 hook 内部接口）**

---

## 📦 快速开始

### cargo 安装

```
cargo install zlm-node
```

### 编译

```
cargo build --release
```

编译完成后，可执行文件位于 `target/release/zlm-node`。

### 生成上报令牌

在部署 zlm-node 和 mgr 之前，用 zlm-node 自己生成一对令牌，避免手工 hash 出错。

**推荐带盐生成**（`b3sum` 命令算不出，更安全）：

```
# 1. 生成一个随机盐
export NODE_TOKEN_SALT=$(openssl rand -hex 32)

# 2. 用盐生成 token
./target/release/zlm-node --gen-token
```

**不带盐生成**（向后兼容旧部署）：

```
./target/release/zlm-node --gen-token
```

输出：

```
NODE_TOKEN=<64位hex 明文>
NODE_REPORT_TOKEN=<64位hex hash>
note: hash computed with NODE_TOKEN_SALT (keyed blake3)   ← 仅在带盐时出现（输出到 stderr）
```

- **`NODE_TOKEN`** → 填到 zlm-node 的 `NODE_TOKEN` 环境变量
- **`NODE_TOKEN_SALT`** → 填到 zlm-node 的 `NODE_TOKEN_SALT` 环境变量（**必须和生成时一致**）
- **`NODE_REPORT_TOKEN`** → 填到 mgr 的 `NODE_REPORT_TOKEN` 环境变量

> ⚠️ **盐只在 zlm-node 侧配置，mgr 侧不需要**。mgr 只存最终 hash，直接比对即可。
>
> ⚠️ **带盐时**：hash 算法是 `blake3::keyed_hash(blake3(salt), token)`，**`b3sum` 命令算不出**——攻击者即使拿到 `NODE_TOKEN` 明文，也必须同时拿到 `NODE_TOKEN_SALT` 才能伪造请求。

### 运行

```
./target/release/zlm-node
```

默认配置会尝试连接 `http://127.0.0.1:9080` 的 ZLM 服务，并向 `http://127.0.0.1:3002/api/zlm/report-status` 上报状态。请通过环境变量覆盖默认值。

---

## ⚙️ 环境变量配置

所有配置均通过环境变量完成，无需配置文件。

### 基础配置

| 变量名 | 默认值 | 必填 | 说明 |
|--------|--------|------|------|
| NODE_TOKEN | 空 | 是 | 节点鉴权令牌（**明文**，上报时内部自动计算 hash，通过 `X-Node-Token` 头发送） |
| NODE_TOKEN_SALT | 无 | 否 | **可选静态盐**。设置后使用 keyed blake3 计算 hash，`b3sum` 命令无法直接算出；**必须和 `--gen-token` 时用的盐一致** |
| SERVER_ID | 无 | 否 | **可选兜底**节点 ID：仅当 `mediaServerId` 拉取失败时使用；未设置时未就绪阶段跳过上报 |
| API_BASE | http://127.0.0.1:9080 | 否 | ZLM API 基础地址 |
| SECRET | 无 | 是 | ZLM API 密钥，必须与 ZLM `config.ini` 的 `[api] secret` 一致 |
| MGR_URL | http://127.0.0.1:3002/api/zlm/report-status | 否 | 管理端状态上报接口 |
| CUSTOM_IP | 无 | 否 | 手动指定公网 IP（优先级最高） |
| INTERFACE | 无 | 否 | 指定网卡名称，从该网卡获取 IPv4 地址 |
| IP_ECHO_API | 无 | 否 | 自定义 IP 获取服务地址，优先级高于内置公共 API |
| ZLM_HTTP_PORT | 9080 | 否 | ZLM HTTP 端口，用于拼接 `http_fmp4_base` |
| USE_HTTPS | false | 否 | 是否使用 HTTPS 协议拼接 `http_fmp4_base` |
| STATIC_BASE | 无 | 否 | 静态资源基地址，若不设置则与 `http_fmp4_base` 相同 |
| HTTP_FMP4_BASE | 无 | 否 | HTTP-FMP4 播放基地址，支持占位符；不设置则自动生成 |
| REPORT_INTERVAL_SECS | 30 | 否 | 上报间隔（秒） |
| IP_REFRESH_INTERVAL_SECS | 5 | 否 | IP 刷新间隔（秒） |
| TLS_ACCEPT_INVALID_CERTS | false | 否 | 是否接受无效 TLS 证书（自签名场景） |
| ENABLE_RTC_EXTERN_IP_UPDATE | true | 否 | 是否自动更新 ZLM 的 `rtc.externIP` |

> **IP 获取优先级**：
> `CUSTOM_IP` > `INTERFACE` > `IP_ECHO_API` > 内置公共 API（`ip.3322.net`、`ip.automate.org.cn`）

### 节点标识与上报行为

- **`mediaServerId`**：启动时从 ZLM `getServerConfig` 拉取，**成功一次后永久缓存**，之后不再请求
- **`version`**：**每次上报前都实测 ZLM `/index/api/version`**
  - 拿到真实版本号 → 上报真实版本（如 `master(a485d89)`），并更新缓存
  - ZLM 挂了（HTTP 请求失败）→ 上报 **`"offline"`**，**上级据此判断 ZLM 离线**
  - ZLM 活着但响应异常 → 优先用缓存兜底，否则上报 `"online (unknown version)"`
- **`SERVER_ID`**（可选兜底）：
  - 未设置：`mediaServerId` 未就绪时跳过本次上报，等下一 tick 重试
  - 已设置：`mediaServerId` 未就绪时暂用该值，拿到后自动切换为 `mediaServerId`
  - **推荐不设置**，避免 mgr 侧短暂出现"错误节点"

### 录像 hook 服务（可选）

启用后，zlm-node 会启动一个本地 HTTP 服务，接管 ZLM 的 `on_record_mp4` 事件，自动将录像上传到 S3 并通知 mgr 写数据库。

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| RECORD_HOOK_ENABLE | false | 是否启用录像 hook 服务 |
| HOOK_LISTEN | 127.0.0.1:3004 | hook 服务监听地址（与 ZLM 同机，用 loopback 即可） |
| MGR_BASE | http://127.0.0.1:3002 | mgr 的 base URL（不含路径），用于调用内部接口 |
| RECORD_S3_ENABLE | false | 是否上传录像到 S3（关闭时只通知 mgr，本地保留） |
| RECORD_KEEP_ON_FAILURE | true | S3 上传失败时是否保留本地文件 |
| RECORD_HOOK_SYNC_INTERVAL_SECS | 60 | 定期兜底重设 ZLM hook 的间隔（秒），最小 10s |

> 录像 hook 调用 mgr 的 `presign-record` / `on-record-event` 内部接口时，**用 `NODE_TOKEN` 认证**（`X-Node-Token` 头），无需单独配 `INTERNAL_API_TOKEN`。

**hook 重试策略**：
- 首次设 hook 失败后按 `2s → 4s → 8s → ... → 30s` 指数退避重试
- 成功后回落到 `RECORD_HOOK_SYNC_INTERVAL_SECS` 稳态检查

---

## 占位符支持

在 `HTTP_FMP4_BASE` 和 `STATIC_BASE` 中可使用以下占位符，程序会在每次上报时替换为实际 IP：

- `{ip}`：替换为当前上报所用的公网 IP（即 `public_ip`）。
- `{interface:网卡名}`：替换为指定网卡的第一个 IPv4 地址，例如 `{interface:eth1}`。

**示例**：

```
# 播放地址使用 eth1 网卡的 IP，而上报 IP 使用公网 IP
export HTTP_FMP4_BASE="http://{interface:eth1}:9080/"

# 播放地址使用公网 IP，但指定 HTTPS 和自定义路径
export HTTP_FMP4_BASE="https://{ip}:9443/live/"
```

---

## 🎥 录像 hook 服务

启用 `RECORD_HOOK_ENABLE=true` 后，zlm-node 会：

1. **启动时**从 ZLM 拉取 `mediaServerId`，用它作为节点标识
2. **启动本地 HTTP 服务**（默认 `127.0.0.1:3004`），接收 ZLM 的 `on_record_mp4` hook
3. **自动重设 ZLM 的 hook 地址**，让 ZLM 录完直接投递到 zlm-node
4. **指数退避兜底重设**（ZLM 慢启动或重启后自动补上）
5. 收到 hook 后：
   - 立即返回 `200`（不阻塞 ZLM）
   - 后台：调 mgr 拿 presigned URL → 流式上传 S3 → 通知 mgr 落库
   - **S3 关闭时**：跳过上传，本地保留，仅通知 mgr

### 节点标识获取优先级（hook 请求内）

1. **hook 请求体自带的 `mediaServerId`**（ZLM 正常一定会带）
2. 共享缓存（由启动时的 `getServerConfig` 或 hook body 填充）
3. 配置的 `SERVER_ID`（可选兜底）
4. 全空 → 返回错误，ZLM 按 `hook.retry` 重试

### 数据流

```
ZLM 录完 → hook → zlm-node (127.0.0.1:3004)
  ↓
zlm-node 立即返回 200
  ↓
后台：
  ① 调 mgr /api/internal/presign-record 拿 S3 presigned URL
  ② 流式 PUT 文件到 S3
  ③ 通知 mgr /api/internal/on-record-event（带 S3 URL 或本地相对路径）
  ↓
mgr 写 DB + 处理客户回调
```

### 启用示例

```
export RECORD_HOOK_ENABLE=true
export HOOK_LISTEN=127.0.0.1:3004
export MGR_BASE=http://mgr.internal:3002
export RECORD_S3_ENABLE=true
export RECORD_KEEP_ON_FAILURE=true
export NODE_TOKEN=<--gen-token 生成的明文>
export NODE_TOKEN_SALT=<生成时用的盐>
./zlm-node
```

启动后日志：

```
[init] detected ZLM mediaServerId = zlmediakit-abc123
[record-hook] listening on 127.0.0.1:3004
[record-hook] set ZLM hook.on_record_mp4 = http://127.0.0.1:3004/hook/on_record_mp4
```

### 本地存储模式（关闭 S3）

```
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

## 🔐 令牌与认证

### zlm-node ↔ mgr 全接口认证

zlm-node **只需要一个 `NODE_TOKEN`** 就能调用 mgr 的所有接口：

| 接口 | 用途 |
|------|------|
| `POST /api/zlm/report-status` | 节点状态上报 |
| `POST /api/internal/presign-record` | 拿 S3 上传 URL |
| `POST /api/internal/on-record-event` | 通知录像落库 |

**两端配置（推荐：带盐）**：

```
zlm-node 侧:  NODE_TOKEN       = 明文 token
              NODE_TOKEN_SALT  = 盐（与 --gen-token 时一致）
              └─ 内部计算 keyed hash: blake3::keyed_hash(blake3(salt), token)
                 → 作为 X-Node-Token 发送

mgr 侧:       NODE_REPORT_TOKEN = 上述 keyed hash 的 hex
              └─ 直接和请求头 X-Node-Token 比对
              （mgr 不需要知道盐）
```

**两端配置（不带盐，向后兼容）**：

```
zlm-node 侧:  NODE_TOKEN          = 明文 token
              └─ 内部普通 blake3(token) → X-Node-Token

mgr 侧:       NODE_REPORT_TOKEN   = blake3(token) 的 hex
```

**生成方式**：

```
# 带盐（推荐）
export NODE_TOKEN_SALT=$(openssl rand -hex 32)
./zlm-node --gen-token

# 不带盐
./zlm-node --gen-token
```

**校验两边一致**：

```
# 看 zlm-node 进程里的明文和盐
cat /proc/$(pgrep -f zlm-node)/environ | tr '\0' '\n' | grep -E '^(NODE_TOKEN|NODE_TOKEN_SALT)='

# 看 mgr 进程里的 hash
cat /proc/$(pgrep -f gbhub-mgr)/environ | tr '\0' '\n' | grep '^NODE_REPORT_TOKEN='
```

> **带盐时 `b3sum` 命令算不出**——`b3sum` 只支持普通 blake3，不支持 keyed 模式。攻击者必须知道算法（keyed blake3）**且**同时拿到明文 token 和盐，才能伪造。
>
> **不带盐时可以用 b3sum 验证**：
> ```
> echo -n "<明文>" | b3sum    # 应等于 mgr 的 hash
> ```

### `INTERNAL_API_TOKEN` 与 zlm-node 无关

`INTERNAL_API_TOKEN` 是 **mgr ↔ sip** 之间的共享令牌，zlm-node 不再需要它，也不需要配置。

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

```
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

```
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

### 最小配置（只上报，不启 hook）

```
export API_BASE=http://127.0.0.1:9080
export SECRET=<你的 ZLM secret>
export MGR_URL=http://mgr.example.com:3002/api/zlm/report-status
export NODE_TOKEN=<--gen-token 生成的明文>
export NODE_TOKEN_SALT=<生成时用的盐>
./zlm-node
```

### 家庭动态公网 IP 场景（推荐使用自建 IP 服务）

```
export NODE_TOKEN=<--gen-token 生成的明文>
export NODE_TOKEN_SALT=<生成时用的盐>
export API_BASE=http://127.0.0.1:9080
export SECRET=your_zlm_secret
export MGR_URL=http://your-gbhub-domain/api/zlm/report-status
export IP_ECHO_API=http://your-cloud-server:8080
export REPORT_INTERVAL_SECS=30
export IP_REFRESH_INTERVAL_SECS=5
export ENABLE_RTC_EXTERN_IP_UPDATE=true
./zlm-node
```

### 完整部署（状态上报 + 录像 hook + S3 上传）

```
# 状态上报
export API_BASE=http://127.0.0.1:9080
export SECRET=your_zlm_secret
export MGR_URL=http://mgr.internal:3002/api/zlm/report-status
export NODE_TOKEN=<--gen-token 生成的明文>
export NODE_TOKEN_SALT=<生成时用的盐>

# 录像 hook
export RECORD_HOOK_ENABLE=true
export HOOK_LISTEN=127.0.0.1:3004
export MGR_BASE=http://mgr.internal:3002
export RECORD_S3_ENABLE=true
export RECORD_KEEP_ON_FAILURE=true

./zlm-node
```

### 手动指定 IP（测试或固定 IP 场景）

```
export CUSTOM_IP=203.0.113.5
# 其他必需变量...
./zlm-node
```

### 使用网卡 IP（多网卡环境）

```
export INTERFACE=eth0
# 其他变量...
./zlm-node
```

---

## 🧪 验证

### 1. 正常上报

确认 zlm-node 日志输出类似：

```
[init] detected ZLM mediaServerId = zlmediakit-abc123
[init] ZLM version: master(a485d89)
Periodic report sent (server_id=zlmediakit-abc123, version=master(a485d89)).
```

### 2. ZLM 未启动

zlm-node 启动时 ZLM 还没起来，会看到：

```
[init] mediaServerId unavailable, report will be skipped until ZLM ready
[init] ZLM is offline (version probe failed)
Skip report: mediaServerId not ready and no SERVER_ID fallback
```

ZLM 起来后最多等一个 `REPORT_INTERVAL_SECS`（默认 30s）就会自动转为正常上报。

### 3. 验证"ZLM 挂了"能否被感知

zlm-node 每次上报前都会实测 ZLM 版本，**ZLM 挂掉后上报的 `version` 会变成 `"offline"`**：

```
# 1. 确认当前版本
redis-cli hget zlm_node:<id> version
# → master(a485d89)

# 2. 停掉 ZLM
systemctl stop mediaserver

# 3. 等一个上报周期（最多 30s + 余量）
sleep 40

# 4. 再看
redis-cli hget zlm_node:<id> version
# → offline

# 5. 恢复 ZLM
systemctl start mediaserver
sleep 40

# 6. 再看
redis-cli hget zlm_node:<id> version
# → master(a485d89)
```

**上级系统只要看 `version` 字段是不是 `"offline"` 就能判断 ZLM 是否离线。**

### 4. rtc.externIP 同步

如果启用了 `ENABLE_RTC_EXTERN_IP_UPDATE`，登录 ZLM 管理界面查看 `rtc.externIP` 是否已同步为公网 IP。

### 5. 录像 hook 就绪

如果启用了录像 hook，看到以下日志说明就绪：

```
[record-hook] listening on 127.0.0.1:3004
[record-hook] set ZLM hook.on_record_mp4 = http://127.0.0.1:3004/hook/on_record_mp4
```

**失败时**会看到：

```
[record-hook] failed to set hook, HTTP 200 code=Some(-1) body={"code":-1,"msg":"secret error"}
```

说明 ZLM 的 `[api] secret` 和 zlm-node 的 `SECRET` 不一致。

### 6. 手工触发录像 hook 测试

```
echo "test" > /tmp/test.mp4
curl -X POST http://127.0.0.1:3004/hook/on_record_mp4 \
  -H "Content-Type: application/json" \
  -d '{
    "file_path": "/tmp/test.mp4",
    "url": "record/test/test.mp4",
    "stream": "34020000001320000001_34020000001310000001",
    "app": "rtp",
    "vhost": "__defaultVhost__",
    "mediaServerId": "zlmediakit-abc123",
    "start_time": 1699000000,
    "time_len": 11.0
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

> **注意**：`version` 字段也跟随上报间隔刷新。ZLM 挂掉后最多等一个 `REPORT_INTERVAL_SECS` 才上报 `"offline"`。想更快感知 ZLM 离线就调小这个值。

### 5. 支持 Docker 部署吗？

完全支持。建议将可执行文件放入容器，并通过 `-e` 参数传入环境变量。

### 6. 录像 hook 启用后，本地文件怎么清理？

- **S3 模式（`RECORD_S3_ENABLE=true`）**：上传成功且 mgr 通知成功后才删本地
- **本地模式（`RECORD_S3_ENABLE=false`）**：文件保留在 ZLM 节点，由 ZLM 自身的录像保留策略清理

### 7. `mediaServerId` 和 `SERVER_ID` 是什么关系？

- zlm-node 启动时**自动从 ZLM 拉 `mediaServerId`**，用它作为节点标识上报给 mgr
- `mediaServerId` **拿到一次后永久缓存**，不再请求 ZLM
- 手工配的 `SERVER_ID` 是**可选兜底**：只在 `mediaServerId` 未就绪时使用；拿到后立刻切换
- **推荐不配 `SERVER_ID`**，让未就绪阶段直接跳过上报，避免 mgr 侧出现错误节点

### 8. zlm-node 需要配置 `INTERNAL_API_TOKEN` 吗？

**不需要。** zlm-node 只用一个 `NODE_TOKEN` 就能调用 mgr 的全部接口：

- `/api/zlm/report-status`（状态上报）
- `/api/internal/presign-record`（拿 S3 上传 URL）
- `/api/internal/on-record-event`（通知录像落库）

`INTERNAL_API_TOKEN` 是 **mgr ↔ sip** 之间共享的，zlm-node 不参与。

### 9. S3 上传失败会丢录像吗？

不会——只要 `RECORD_KEEP_ON_FAILURE=true`（默认），S3 上传失败时本地文件会保留。可以：
- 排查 S3 / 网络问题
- 手动或脚本重传
- 临时关闭 S3（`RECORD_S3_ENABLE=false`）让 zlm-node 走本地模式

### 10. `--gen-token` 和手工用 `b3sum` 有什么区别？

**带盐（设置了 `NODE_TOKEN_SALT`）时**：

- `--gen-token` 内部用 `blake3::keyed_hash(blake3(salt), token)`；
- **`b3sum` 命令完全不支持 keyed 模式**，无论如何都算不出正确 hash；
- 攻击者必须知道算法、知道盐、知道明文，才能算出正确 hash。

**不带盐时**：

- 与 `b3sum` 兼容：`echo -n "<明文>" | b3sum` 应该等于 `NODE_REPORT_TOKEN`；
- 手工用 `b3sum` 时容易踩 `echo -n` 缺换行、`\r` 混入、编码不一致等坑，导致两边算出的 hash 不同、永远 401。

**推荐**：始终带盐，用 `--gen-token` 生成。

### 11. `NODE_TOKEN_SALT` 有什么用？

`NODE_TOKEN_SALT` 是一个**静态盐**，让 hash 从普通 blake3 变成 keyed blake3：

```
无盐：  hash = blake3(token)
带盐：  hash = blake3::keyed_hash(blake3(salt), token)
```

**优势**：

- **`b3sum` 命令无法算出** —— `b3sum` 只支持无 key 模式；
- **仅泄漏 `NODE_TOKEN` 无法伪造** —— 攻击者还得同时拿到 salt；
- **mgr 侧不需要知道盐** —— 只存最终 hash，比对逻辑不变。

**注意**：

- **盐必须和 `--gen-token` 时一致**——改了盐等于改了 hash，两边对不上；
- **盐和 token 应尽量分开存放**（比如 salt 放单独的 `chmod 600` 文件）——同时泄漏两者等于没加盐；
- **盐不是万能的**——进程内存被 dump 时仍然拿得到。

### 12. `version` 字段怎么判断 ZLM 是否离线？

**每次上报前 zlm-node 都会实测 ZLM `/index/api/version`**，把结果作为 `version` 字段上报：

| ZLM 状态 | 上报的 `version` | 上级判断 |
|---|---|---|
| 正常 | `master(a485d89)` 等真实版本 | 在线 |
| 活着但响应异常 | 缓存的历史版本（或 `online (unknown version)`） | 在线 |
| **挂了（HTTP 请求失败）** | **`offline`** | **离线** |
| 恢复 | 又变回真实版本 | 在线 |

**上级系统只要判断 `version == "offline"` 就知道 ZLM 挂了。**

**感知延迟**：最多一个 `REPORT_INTERVAL_SECS`（默认 30s）。想更快就调小它。

### 13. ZLM 慢启动时 zlm-node 会怎样？

自动自愈，无需干预：

| 组件 | 失败表现 | 自愈方式 | 最坏延迟 |
|---|---|---|---|
| hook 设置 | 首次 set 失败 | 指数退避 2s→4s→…→30s | ZLM 起来后 ≤30s |
| `mediaServerId` | 未就绪 | 每次上报前重试 | 起来后 ≤`IP_REFRESH_INTERVAL_SECS`（5s） |
| `version` | 上报 `"offline"` | 每次上报前重试 | 起来后 ≤`REPORT_INTERVAL_SECS`（30s） |
| 状态上报 | 跳过（未配 `SERVER_ID`） | ZLM 起来后自动开始 | 5~30s |
| hook 回调处理 | 不依赖 zlm-node 状态 | 直接用 body 里的 `mediaServerId` | 无 |

### 14. hook 设置"看起来成功但没生效"怎么办？

zlm-node 已经校验了 **HTTP 200 且 ZLM 返回 `code == 0`**，只有两者都满足才算成功。如果日志显示：

```
[record-hook] failed to set hook, HTTP 200 code=Some(-1) body={"code":-1,"msg":"secret error"}
```

说明 ZLM 拒绝了（通常是 `secret` 错）。检查 zlm-node 的 `SECRET` 和 ZLM `config.ini` 的 `[api] secret` 是否一致。

**`changed == 0` 不是失败**——它只表示"值未变化"（比如重复设置同一个 hook），`code == 0` 才是成功标志。

### 15. 多机部署时 `NODE_TOKEN` 能共用吗？

**可以，但不推荐。**

mgr 侧的 `NODE_REPORT_TOKEN` 目前是单值——所有 zlm-node 必须共用同一个 `NODE_TOKEN`，一台泄漏 = 全部沦陷。

要支持"每台独立 token"，mgr 侧的 `NODE_REPORT_TOKEN` 需要改成逗号分隔的多值：

```
NODE_REPORT_TOKEN=hash1,hash2,hash3
```

对应 mgr 代码要把"直接比对"改成"遍历比对"。当前版本未实现。

---

## 📄 许可证

本项目采用 [MIT License](LICENSE)。
