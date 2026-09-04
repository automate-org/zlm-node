# zlm-node

**zlm-node** 是一个用于将 ZLMediaKit 节点状态上报到管理平台（如 GBHub）的轻量级代理程序。它自动获取公网 IP、更新 ZLM 的 `rtc.externIP`、并周期性上报节点信息，特别针对家庭动态公网 IP 场景进行了优化。

---

## ✨ 功能特性

- ✅ 自动获取公网 IP（支持多种来源，支持自定义 IP 服务）
- ✅ 周期性上报节点状态到管理平台（`MGR_URL`）
- ✅ 自动更新 ZLM 的 `rtc.externIP`（可选）
- ✅ 支持手动指定 IP 或网卡 IP
- ✅ 支持自定义 IP 获取服务地址（`IP_ECHO_API`），摆脱第三方依赖
- ✅ 内置公共 IP 查询 API 作为后备，确保高可用
- ✅ 支持 TLS 证书校验开关（自签名场景）
- ✅ 低资源占用，适合嵌入式或家庭设备部署

---

## 📦 快速开始

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

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `NODE_TOKEN` | 空 | 节点鉴权令牌（上报时通过 `X-Node-Token` 头发送 blake3 哈希） |
| `SERVER_ID` | `1` | 节点 ID，用于标识当前 ZLM 节点 |
| `API_BASE` | `http://127.0.0.1:9080` | ZLM API 基础地址 |
| `SECRET` | `935v73f7-bb6b-4889-a715-d9eb2d1936aa` | ZLM API 密钥 |
| `MGR_URL` | `http://127.0.0.1:3002/api/zlm/report-status` | 管理端状态上报接口 |
| `CUSTOM_IP` | 无 | 手动指定公网 IP（优先级最高） |
| `INTERFACE` | 无 | 指定网卡名称，从该网卡获取 IPv4 地址 |
| `IP_ECHO_API` | 无 | 自定义 IP 获取服务地址，例如 `http://your-cloud-server:8080`，优先级高于内置公共 API |
| `ZLM_HTTP_PORT` | `9080` | ZLM HTTP 端口，用于拼接 `http_fmp4_base` |
| `USE_HTTPS` | `false` | 是否使用 HTTPS 协议拼接 `http_fmp4_base` |
| `STATIC_BASE` | 无 | 静态资源基地址，若不设置则与 `http_fmp4_base` 相同 |
| `HTTP_FMP4_BASE` | 无 | 手动指定 `http_fmp4_base`，覆盖自动生成 |
| `REPORT_INTERVAL_SECS` | `30` | 上报间隔（秒） |
| `TLS_ACCEPT_INVALID_CERTS` | `false` | 是否接受无效 TLS 证书（自签名场景） |
| `ENABLE_RTC_EXTERN_IP_UPDATE` | `true` | 是否自动更新 ZLM 的 `rtc.externIP` |

> **IP 获取优先级**：  
> `CUSTOM_IP` > `INTERFACE` > `IP_ECHO_API` > 内置公共 API（`ip.3322.net`、`ip.automate.org.cn`）

---

## 🔧 配置示例

### 家庭动态公网 IP 场景（推荐使用自建 IP 服务）

假设您已在云服务器部署了 IP 身份服务（返回纯 IP 文本），地址为 `http://your-cloud-server:8080`。

在 zlm-node 的运行环境中设置：

```bash
export NODE_TOKEN=your_secret_token_here
export SERVER_ID=home-node-01
export API_BASE=http://127.0.0.1:9080
export SECRET=your_zlm_secret
export MGR_URL=http://your-gbhub-domain/api/zlm/report-status
export IP_ECHO_API=http://your-cloud-server:8080
export REPORT_INTERVAL_SECS=30
export ENABLE_RTC_EXTERN_IP_UPDATE=true
```

然后启动：

```bash
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
   ```bash
   Public IP: 123.45.67.89, Version: master(abc123)
   Report sent successfully.
   ```
2. 检查管理端是否收到节点状态更新。
3. 如果启用了 `ENABLE_RTC_EXTERN_IP_UPDATE`，登录 ZLM 管理界面查看 `rtc.externIP` 是否已同步为公网 IP。

---

## 🛠️ 常见问题

### 1. 为什么获取到的 IP 不正确？
- 如果使用了 `CUSTOM_IP`，请检查设置的值。
- 如果使用了 `INTERFACE`，确保该网卡存在且配置了 IPv4。
- 如果使用了 `IP_ECHO_API`，确保该服务可从 zlm-node 所在网络访问，且返回纯 IP 文本（可包含换行）。
- 若以上都未设置，则使用内置公共 API，可能受网络环境影响。

### 2. 如何彻底禁用内置公共 API？
- 将 `IP_ECHO_API` 始终设置为您的自建服务，并在网络层面阻止对 `ip.3322.net` 等域名的访问。代码层面保留后备逻辑不影响正常使用。

### 3. 自建 IP 服务如何实现？
- 任何能返回客户端公网 IP 的 HTTP 服务均可，例如使用 Nginx 的 `return 200 "$remote_addr";`，或运行一个极简的 Rust/Python 程序。

### 4. 上报间隔调多少合适？
- 家庭宽带 IP 变化不频繁，默认 `30` 秒已足够。如需更快感知 IP 变化，可适当降低至 `10` 秒。

### 5. 支持 Docker 部署吗？
- 完全支持。建议将可执行文件放入容器，并通过 `-e` 参数传入环境变量。

---

## 📄 许可证

本项目采用 [MIT License](LICENSE)。
