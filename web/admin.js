'use strict'
async function renderAdminSettings(box) {
  try {
    const response = await fetch('/auth/status', { credentials:'same-origin', cache:'no-store' })
    if (!response.ok || !box.isConnected) return
    const status = await response.json()
    if (!box.isConnected) return
    if (!status.signedIn) { location.replace('/login'); return }
    const credentials = box.querySelector('#admin-credentials')
    credentials.innerHTML = `<details class="admin-password"><summary><span><strong>修改用户名或密码</strong><small class="muted">修改后所有设备需要重新登录</small></span></summary>
      <form id="admin-password-form" autocomplete="off" class="admin-form"><label class="full" for="admin-username">用户名<input data-auth-secret id="admin-username" type="text" autocomplete="username" maxlength="64" required autocapitalize="none" spellcheck="false"></label>
      <label for="admin-password">新密码<input data-auth-secret id="admin-password" type="password" autocomplete="new-password" minlength="6" maxlength="256" placeholder="留空则不修改"></label>
      <label for="admin-confirm">确认新密码<input data-auth-secret id="admin-confirm" type="password" autocomplete="new-password" maxlength="256"></label>
      <label class="full" for="admin-current">当前密码<input data-auth-secret id="admin-current" type="password" autocomplete="current-password" maxlength="256" required placeholder="验证身份"></label>
      <p id="admin-error" class="warn-text full" role="alert" hidden></p><div class="form-actions full"><button class="primary" type="submit">保存并重新登录</button></div></form></details>`
    const form = credentials.querySelector('form'), error = credentials.querySelector('#admin-error')
    const currentUsername = status.username || 'admin'
    box.querySelector('#admin-identity').textContent = currentUsername
    const display = box.querySelector('#p-display')
    display.dataset.fallback = currentUsername
    if (!display.textContent.trim()) display.textContent = currentUsername
    form.querySelector('#admin-username').value = currentUsername
    form.onsubmit = async event => {
      event.preventDefault()
      error.hidden = true
      const password = form.querySelector('#admin-password').value
      const username = form.querySelector('#admin-username').value.trim()
      if (!/^[\p{L}\p{N}._-]{1,64}$/u.test(username)) { error.textContent = '用户名须为 1–64 个字符，可用中文、字母、数字、点、下划线和短横线。'; error.hidden = false; return }
      if (username === currentUsername && !password) { error.textContent = '请修改用户名或填写新密码。'; error.hidden = false; return }
      if (password !== form.querySelector('#admin-confirm').value) { error.textContent = '两次输入的新密码不一致。'; error.hidden = false; return }
      if (password && [...password].length < 6) { error.textContent = '新密码至少需要 6 个字符。'; error.hidden = false; return }
      if (new TextEncoder().encode(password).length > 256) { error.textContent = '密码不能超过 256 字节。'; error.hidden = false; return }
      const button = form.querySelector('button'); button.disabled = true
      try {
        const response = await fetch('/auth/password', { method:'POST', credentials:'same-origin', headers:{'content-type':'application/json'}, body:JSON.stringify({current:form.querySelector('#admin-current').value,username,...(password?{password}:{})}) })
        if (response.status === 401) throw new Error('当前密码不正确或登录已失效。')
        const result = await response.json().catch(() => ({}))
        if (!response.ok || result.ok !== true) throw new Error(result.error || '修改失败，请稍后重试。')
        form.reset(); sessionStorage.removeItem('dotbot.upgrade-drafts'); location.replace('/login')
      } catch (failure) { error.textContent = failure instanceof TypeError ? '网络连接中断，请重试。' : failure.message; error.hidden = false }
      finally { button.disabled = false }
    }
    const logout = box.querySelector('#admin-logout'); logout.hidden = false
    logout.onclick = async event => {
      const button = event.currentTarget; button.disabled = true
      try {
        const response = await fetch('/auth/logout', { method:'POST', credentials:'same-origin' })
        if (response.status !== 401 && (!response.ok || (await response.json().catch(() => ({}))).ok !== true)) throw new Error('退出失败，请重试。')
        sessionStorage.removeItem('dotbot.upgrade-drafts'); location.replace('/login')
      } catch (failure) { error.textContent = failure instanceof TypeError ? '网络连接中断，请重试。' : failure.message; error.hidden = false; credentials.querySelector('details').open = true; button.disabled = false }
    }
  } catch { const slot = box.querySelector('#admin-credentials'); if (box.isConnected && slot) slot.innerHTML = '<p class="muted admin-load-error">账号状态加载失败，请重新打开设置。</p>' }
}

let signingOut = false
async function goToAdminLogin() {
  if (signingOut) return
  signingOut = true
  try {
    const response = await fetch('/auth/status', { credentials:'same-origin', cache:'no-store' })
    if (response.ok) {
      const status = await response.json()
      if (!status.signedIn) location.replace('/login')
    }
  } catch {} finally { signingOut = false }
}
addEventListener('pageshow', event => { if (event.persisted) void goToAdminLogin() })
