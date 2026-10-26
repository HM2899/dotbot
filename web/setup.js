'use strict'
;(() => {
  const $ = selector => document.querySelector(selector)
  const form = $('#setup-form'), error = $('#setup-error'), submit = $('#setup-submit')
  let step = 0, busy = false
  const showError = message => { error.textContent = message; error.hidden = false }
  form.addEventListener('input', () => { error.hidden = true })
  const render = () => {
    $('#account-step').hidden = step !== 0
    $('#profile-step').hidden = step !== 1
    $('#account-step').disabled = busy
    $('#profile-step').disabled = busy
    $('#setup-back').hidden = step !== 1
    $('#setup-back').disabled = busy
    $('#setup-logout').disabled = busy
    submit.disabled = busy
    submit.querySelector('span').textContent = busy ? '正在保存…' : step === 0 ? '下一步' : '完成设置并登录'
    $('#setup-progress').textContent = step === 0 ? '第 1 步，共 2 步 · 管理员账号' : '第 2 步，共 2 步 · 个人信息'
    $('#setup-intro').textContent = step === 0 ? '先设置专属管理员账号，开始与你的团队协作。' : '让团队认识你，之后也可以在设置中调整。'
    form.setAttribute('aria-busy', String(busy))
  }
  const accountValid = () => {
    const username = $('#setup-username').value.trim(), password = $('#setup-password').value
    if (!/^[\p{L}\p{N}._-]{1,64}$/u.test(username)) { showError('用户名须为 1–64 个字符，可用中文、字母、数字、点、下划线和短横线。'); return false }
    if ([...password].length < 6 || new TextEncoder().encode(password).length > 256 || !password.trim()) { showError('新密码须至少 6 个字符，且不超过 256 字节。'); return false }
    if (password !== $('#setup-confirm').value) { showError('两次输入的新密码不一致。'); return false }
    return true
  }
  $('#setup-back').onclick = () => { step = 0; error.hidden = true; render(); $('#setup-username').focus() }
  $('#setup-logout').onclick = async () => {
    if (busy) return
    busy = true; render()
    try {
      const response = await fetch('/auth/logout', {method:'POST',credentials:'same-origin'})
      if (!response.ok && response.status !== 401) throw new Error('退出失败，请重试。')
      form.reset(); location.replace('/login')
    } catch { showError('退出失败，请检查网络后重试。'); busy = false; render() }
  }
  form.onsubmit = async event => {
    event.preventDefault()
    if (busy) return
    error.hidden = true
    if (!accountValid()) { step = 0; render(); return }
    if (step === 0) { step = 1; render(); $('#setup-name').focus(); return }
    const name = $('#setup-name').value.trim(), bio = $('#setup-bio').value.trim()
    if (!name || [...name].length > 40 || [...bio].length > 500) { showError('昵称须为 1–40 个字符，个人简介不超过 500 个字符。'); return }
    busy = true; render()
    try {
      const response = await fetch('/auth/setup', {method:'POST',credentials:'same-origin',headers:{'content-type':'application/json'},body:JSON.stringify({username:$('#setup-username').value.trim(),password:$('#setup-password').value,name,bio})})
      if (response.status === 401 || response.status === 409) { form.reset(); location.replace('/login'); return }
      const result = await response.json().catch(() => ({}))
      if (!response.ok || result.ok !== true) throw new Error(result.error || '设置失败，请稍后重试。')
      form.reset(); location.replace('/login?welcome=1')
    } catch (failure) { showError(failure instanceof TypeError ? '网络连接中断，请重试。' : failure.message) }
    finally { busy = false; render() }
  }
  const load = async () => {
    try {
      const response = await fetch('/auth/status', {credentials:'same-origin',cache:'no-store'})
      if (!response.ok) throw new Error()
      const status = await response.json()
      if (!status.signedIn) { location.replace('/login'); return }
      if (!status.setupRequired) { location.replace('/'); return }
      render()
    } catch { showError('暂时无法连接服务，请刷新页面重试。') }
  }
  addEventListener('pageshow', event => { if (event.persisted) void load() })
  void load()
})()
