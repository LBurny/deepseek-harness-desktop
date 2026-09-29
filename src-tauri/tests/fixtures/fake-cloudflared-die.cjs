// Mock cloudflared（必死款）：供隧道监督降级重试的集成测试使用。
// 永远不打印 trycloudflare URL，启动后立刻以非零码退出——每轮 spawn 都命中
// 监督循环的"exited before up"失败点，用于驱动连续失败降级。
const fs = require('node:fs')
const path = require('node:path')

console.log('2026-08-17T00:00:00Z INF Starting tunnel tunnelID=fake-die')

// 工作目录下存在 fake-cloudflared.die-delay 文件时，按其毫秒数延迟退出
// （用于驱动 up_timeout 失败点；缺省立即退出）。
const delayMarker = path.join(process.cwd(), 'fake-cloudflared.die-delay')
if (fs.existsSync(delayMarker)) {
  const ms = Number(fs.readFileSync(delayMarker, 'utf8').trim())
  if (ms > 0) {
    setTimeout(() => process.exit(1), ms)
    setInterval(() => {}, 10000)
  } else {
    process.exit(1)
  }
} else {
  process.exit(1)
}
