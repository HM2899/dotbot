'use strict'
;(() => {
  const $ = selector => document.querySelector(selector)
  const form = $('#login-form'), error = $('#login-error'), submit = $('#login-submit')
  let waiting = false
  const showError = message => { error.textContent = message; error.hidden = false }
  $('#toggle-password').onclick = () => {
    const input = $('#password'), showing = input.type === 'password'
    input.type = showing ? 'text' : 'password'
    $('#toggle-password').setAttribute('aria-label', showing ? '隐藏密码' : '显示密码')
    $('#toggle-password').setAttribute('aria-pressed', String(showing))
  }
  const load = async () => {
    try {
      const response = await fetch('/auth/status', { credentials:'same-origin', cache:'no-store' })
      if (!response.ok) throw new Error()
      const status = await response.json()
      if (status.signedIn) { location.replace(status.setupRequired ? '/setup' : '/'); return }
      if (status.setupRequired) {
        $('#login-heading').textContent = '开始设置 dotbot'
        $('.login-intro').textContent = '登录后继续完成设置。'
        if (!$('#username').value) $('#username').value = 'admin'
      } else if (new URLSearchParams(location.search).has('welcome')) {
        $('#login-heading').textContent = '设置完成'
        $('.login-intro').textContent = '使用刚刚设置的管理员账号和新密码登录。'
      }
      for (const field of form.elements) field.disabled = false
      submit.querySelector('span').textContent = '登录'
      form.setAttribute('aria-busy', 'false')
    } catch {
      showError('暂时无法连接服务，请刷新页面重试。')
      submit.querySelector('span').textContent = '重新连接'
      submit.type = 'button'; submit.disabled = false; submit.onclick = () => location.reload()
    }
  }
  form.onsubmit = async event => {
    event.preventDefault()
    if (waiting) return
    waiting = true; error.hidden = true; submit.disabled = true
    form.setAttribute('aria-busy', 'true'); submit.querySelector('span').textContent = '正在登录…'
    try {
      const response = await fetch('/auth/login', { method:'POST', credentials:'same-origin', headers:{'content-type':'application/json'}, body:JSON.stringify({username:$('#username').value.trim(),password:$('#password').value,remember:$('#remember').checked}) })
      const result = await response.json().catch(() => ({}))
      if (!response.ok || result.ok !== true) throw new Error(result.error || '登录失败，请稍后重试。')
      $('#password').value = ''
      location.replace(result.setupRequired ? '/setup' : '/')
    } catch (failure) {
      showError(failure instanceof TypeError ? '网络连接中断，请重试。' : failure.message)
      $('#password').focus()
    } finally {
      waiting = false; submit.disabled = false; submit.querySelector('span').textContent = '登录'; form.setAttribute('aria-busy', 'false')
    }
  }
  // Settings use same-document history entries. Keep back navigation on the
  // login document canonical after the authenticated document is replaced.
  addEventListener('popstate', () => { history.replaceState(null, '', '/login'); void load() })
  addEventListener('pageshow', event => { if (event.persisted) void load() })
  void load()
})()
