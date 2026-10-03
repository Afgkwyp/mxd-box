import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { initTheme } from './lib/theme'
import { initMotion } from './lib/motion'

// 开发用：URL 带 `mock` 时装上 Tauri 模拟层，普通浏览器里也能看界面（生产构建里这段整个被摘掉）
if (import.meta.env.DEV && new URLSearchParams(location.search).has('mock')) {
  await import('./dev/tauriMock')
}

// 先应用已保存的主题（并建立跨窗口同步），再渲染 —— 否则日间主题会先闪一下深色
initTheme()
// 动效开关同理：第一屏的入场动画就要按用户的设置来
initMotion()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
