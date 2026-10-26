// Loaded in <head>, before the page draws, so a chosen theme never flashes the other one.
'use strict'
;(() => {
  // Migrate preferences and unsent upgrade drafts once, before either page draws.
  for (const [storage, keys] of [[localStorage, ['theme', 'lang']], [sessionStorage, ['upgrade-drafts']]]) {
    for (const key of keys) {
      const previous = storage.getItem(`teambot.${key}`)
      if (previous !== null && storage.getItem(`dotbot.${key}`) === null) storage.setItem(`dotbot.${key}`, previous)
      storage.removeItem(`teambot.${key}`)
    }
  }
  const theme = localStorage.getItem('dotbot.theme')
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme
})()
