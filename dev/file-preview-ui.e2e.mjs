#!/usr/bin/env node
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { execFileSync } from 'node:child_process'
import { readFileSync, mkdirSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import { join } from 'node:path'
import { startServer, until, root } from './harness.mjs'
const vendorScope={module:{exports:{}},exports:{},TextEncoder,TextDecoder,Uint8Array}
runInNewContext(readFileSync(join(root,'web/vendor/fflate-0.8.2.js'),'utf8'),vendorScope)
const {zipSync,strToU8}=vendorScope.module.exports
const zip = entries => zipSync(Object.fromEntries(Object.entries(entries).map(([name, text]) => [name, strToU8(text)])))
let app, server, browser, context, page, owned=[]
const descendants=pid=>{const rows=execFileSync('ps',['-eo','pid=,ppid=']).toString().trim().split('\n').map(r=>r.trim().split(/\s+/).map(Number)),ids=new Set([pid]);let n;do{n=ids.size;for(const[id,parent]of rows)if(ids.has(parent))ids.add(id)}while(n!==ids.size);return [...ids]}
const running=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
try {
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  const {call}=app
  await call('settings.lang',{lang:'zh'});await call('settings.learning',{enabled:false})
  const chief=(await call('team.state')).bots[0], conversation=chief.conversation
  const docx=zip({'word/document.xml':'<w:document xmlns:w="urn:word"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>文档标题</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>加粗正文</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>单元格内容</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>'})
  const xlsx=zip({
    'xl/workbook.xml':'<workbook xmlns:r="urn:rel"><sheets><sheet name="销售数据" r:id="r1"/><sheet name="第二表" r:id="r2"/></sheets></workbook>',
    'xl/_rels/workbook.xml.rels':'<Relationships><Relationship Id="r1" Target="worksheets/sheet1.xml"/><Relationship Id="r2" Target="/xl/worksheets/sheet2.xml"/></Relationships>',
    'xl/sharedStrings.xml':'<sst><si><t>产品名称</t></si></sst>',
    'xl/worksheets/sheet1.xml':'<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>金额</t></is></c></row><row><c r="A2" t="inlineStr"><is><t>设备 A</t></is></c><c r="C2"><v>123.5</v></c></row></sheetData></worksheet>',
    'xl/worksheets/sheet2.xml':'<worksheet><sheetData><row><c r="A1" t="inlineStr"><is><t>另一张表</t></is></c></row></sheetData></worksheet>'
  })
  const pptx=zip({
    'ppt/presentation.xml':'<p:presentation xmlns:p="urn:p" xmlns:r="urn:r"><p:sldIdLst><p:sldId r:id="s2"/><p:sldId r:id="s1"/></p:sldIdLst></p:presentation>',
    'ppt/_rels/presentation.xml.rels':'<Relationships><Relationship Id="s1" Target="slides/slide1.xml"/><Relationship Id="s2" Target="slides/slide2.xml"/></Relationships>',
    'ppt/slides/slide1.xml':'<sld><p><r><t>后显示的幻灯片</t></r></p></sld>',
    'ppt/slides/slide2.xml':'<sld><p><r><t>先显示的幻灯片</t></r></p></sld>'
  })
  const odt=zip({'content.xml':'<office:document xmlns:office="urn:o" xmlns:text="urn:t"><office:body><office:text><text:h>开放文档标题</text:h><text:p>开放文档正文</text:p></office:text></office:body></office:document>'})
  const ods=zip({'content.xml':'<office:document xmlns:office="urn:o" xmlns:table="urn:table" xmlns:text="urn:t"><office:spreadsheet><table:table table:name="库存"><table:table-row><table:table-cell table:number-columns-repeated="2"><text:p>重复单元格</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:document>'})
  const odp=zip({'content.xml':'<office:document xmlns:office="urn:o" xmlns:draw="urn:d" xmlns:text="urn:t"><office:presentation><draw:page draw:name="开放幻灯片"><text:p>演示正文</text:p></draw:page></office:presentation></office:document>'})
  const fixtures = {
    '项目说明.md':'# 项目说明\n\n**加粗内容**\n\n| 姓名 | 状态 |\n| --- | --- |\n| 小明 | 完成 |\n\n- [x] 已完成\n\n```python\nprint("你好")\n```\n\n<script>window.__previewPwn=1</script>',
    '数据.csv':'姓名,备注\r\n张三,"含,逗号"\r\n李四,"多行\n备注"\r\n',
    '配置.json':'{"name":"中文","nested":{"ok":true},"html":"<script>window.__previewPwn=1</script>"}',
    '示例.rs':'fn main() {\n    println!("你好");\n}',
    '页面.html':'<!doctype html><html><head><style>h1{color:rgb(0,128,0)}</style></head><body><h1>网页内容</h1><script>parent.__previewPwn=1;window.__framePwn=1</script><img src="https://preview.invalid/pixel"><a href="https://preview.invalid/nav">外链</a></body></html>',
    '图形.svg':'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" onload="parent.__previewPwn=1"><script>parent.__previewPwn=1</script><circle cx="50" cy="50" r="40" fill="red"/></svg>',
    '说明.docx':docx, '工作簿.xlsx':xlsx, '演示.pptx':pptx, '文档.odt':odt, '表格.ods':ods, '演示.odp':odp,
    '资料.zip':zip({'a.txt':'hello','路径/b.md':'# Markdown'}),
    '未知.bin':Buffer.from([0,1,2,3]), '损坏.docx':Buffer.from('invalid zip'),
    '大文件.md':'# 超长文档\n\n'+ '正文内容\n'.repeat(100000),
    'GBK.txt':Buffer.from([0xd6,0xd0,0xce,0xc4]),
    'UTF16.txt':Buffer.concat([Buffer.from([0xff,0xfe]),Buffer.from('中文 UTF16 内容','utf16le')]),
    '制表数据.tsv':'姓名\t值\n小红\t42',
    '超限.docx':zip({'word/document.xml':'x'.repeat(2*1024*1024+1)})
  }
  const files={}
  for(const [name,body]of Object.entries(fixtures)) {
    const response=await fetch(`${app.base}/api/files?${new URLSearchParams({conversation,name})}`,{method:'POST',headers:{cookie:app.cookie,'content-type':'application/octet-stream'},body})
    assert.equal(response.status,200);files[name]=(await response.json()).file
  }
  const all=Object.values(files)
  for(let i=0;i<all.length;i+=8)await call('chat.send',{conversation,text:'查看这些附件',files:all.slice(i,i+8).map(f=>f.src)})
  await until('chat idle',async()=>!(await call('team.state')).work.runs[conversation])
  server=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined});owned=descendants(server.process().pid)
  browser=await chromium.connect(server.wsEndpoint());context=await browser.newContext({viewport:{width:1280,height:900},locale:'zh-CN'});page=await context.newPage()
  page.setDefaultTimeout(10000)
  const errors=[], external=[];page.on('pageerror',e=>errors.push(e.message))
  await page.route('https://preview.invalid/**',route=>{external.push(route.request().url());return route.abort()})
  await app.authorize(context); await page.goto(app.address);await page.waitForFunction(()=>view.team&&view.current&&!view.conversationLoading)
  const dialog=page.locator('#file-preview-dialog'), body=page.locator('#file-preview-body')
  const close=async()=>{await dialog.locator('[data-preview-close]').click();await dialog.waitFor({state:'hidden'})}
  const preview=async name=>{
    const card=page.locator('#messages .user-file').filter({hasText:name}).first()
    await card.locator('[data-file-preview]').click()
    await dialog.waitFor({state:'visible'});await page.waitForFunction(()=>document.querySelector('#file-preview-body')?.getAttribute('aria-busy')==='false')
  }
  await preview('项目说明.md')
  assert.equal(await body.locator('.md-h1').textContent(),'项目说明')
  assert.equal(await body.locator('strong').textContent(),'加粗内容')
  assert.equal(await body.locator('table td').first().textContent(),'小明')
  assert.equal(await body.locator('input[type=checkbox]:checked').count(),1)
  assert.ok((await body.locator('code').textContent()).includes('print("你好")'))
  assert.equal(await body.locator('script').count(),0)
  await dialog.locator('[data-preview-mode="source"]').click();assert.ok((await body.textContent()).includes('# 项目说明'))
  await dialog.locator('[data-preview-mode="rendered"]').click()
  const artifacts=join(root,'test-results');mkdirSync(artifacts,{recursive:true})
  await page.screenshot({path:join(artifacts,'file-markdown-desktop.png'),animations:'disabled'})
  await page.setViewportSize({width:390,height:844});assert.ok(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth+1))
  await page.screenshot({path:join(artifacts,'file-markdown-mobile.png'),animations:'disabled'})
  await page.keyboard.press('Escape');assert.equal(await dialog.isVisible(),false)
  await page.setViewportSize({width:1280,height:900})
  console.log('ok   uploaded Markdown renders headings/tables/tasks/code; source mode, escaping, desktop/mobile and Escape work')
  await preview('数据.csv');assert.equal(await body.locator('tr').count(),3);assert.equal(await body.locator('tr').nth(1).locator('td').nth(1).textContent(),'含,逗号');assert.equal(await body.locator('tr').nth(2).locator('td').nth(1).textContent(),'多行\n备注');await close()
  await preview('制表数据.tsv');assert.equal(await body.locator('tr').nth(1).locator('td').nth(1).textContent(),'42');await close()
  await preview('配置.json');assert.ok((await body.textContent()).includes('\n  "name": "中文"'));assert.equal(await body.locator('script').count(),0);await close()
  await preview('示例.rs');assert.ok((await body.textContent()).includes('fn main()'));await close()
  await preview('UTF16.txt');assert.ok((await body.textContent()).includes('中文 UTF16 内容'));await close()
  await preview('GBK.txt');assert.equal(await body.locator('pre').textContent(),'中文');await close()
  console.log('ok   CSV quoted commas/newlines, TSV, formatted JSON, code and UTF-16 Chinese text render')
  for(const name of ['页面.html','图形.svg']) {
    await preview(name)
    const frame=body.locator('iframe');assert.equal(await frame.getAttribute('sandbox'),'')
    const frameContent=page.frameLocator('#file-preview-body iframe')
    if(name.endsWith('html'))await frameContent.locator('h1').waitFor()
    else await frameContent.locator('circle').waitFor()
    assert.equal(await frameContent.locator('script').count(),0)
    assert.equal(await page.evaluate(()=>window.__previewPwn),undefined)
    await close()
  }
  assert.deepEqual(external,[])
  console.log('ok   HTML/SVG show visual content without scripts, parent access or external resource requests')
  await preview('说明.docx');assert.equal(await body.locator('h1').textContent(),'文档标题');assert.equal(await body.locator('strong').textContent(),'加粗正文');assert.equal(await body.locator('td').textContent(),'单元格内容');await close()
  await preview('工作簿.xlsx');assert.equal(await dialog.locator('select option').count(),2);assert.ok((await body.textContent()).includes('设备 A'));assert.equal(await body.locator('tr').nth(1).locator('td').nth(1).textContent(),'');await dialog.locator('select').selectOption('1');assert.ok((await body.textContent()).includes('另一张表'));await close()
  await preview('演示.pptx');assert.ok((await body.textContent()).includes('先显示的幻灯片'));await dialog.locator('select').selectOption('1');assert.ok((await body.textContent()).includes('后显示的幻灯片'));await close()
  for(const [name,text]of [['文档.odt','开放文档正文'],['表格.ods','重复单元格'],['演示.odp','演示正文'],['资料.zip','路径/b.md']]){await preview(name);assert.ok((await body.textContent()).includes(text));await close()}
  console.log('ok   DOCX text/styles/tables, XLSX sheet switching/sparse cells, PPTX slide order, ODF and ZIP directory render locally')
  await preview('大文件.md');assert.ok((await body.textContent()).includes('预览内容已截断'));assert.ok((await body.textContent()).includes('正文内容'));await close()
  for(const name of ['未知.bin','损坏.docx','超限.docx']){await preview(name);assert.equal(await body.locator('[role=alert]').count(),1);await close()}
  // Generated Bot files also render inline, and streaming updates preserve loaded DOM.
  await page.evaluate(file=>{view.items.push({...file,id:'generated-preview',kind:'file',bot:view.team.bots[0].id});renderMessages({keepScroll:true})},files['项目说明.md'])
  const inline=page.locator('[data-id="generated-preview"] .file-inline')
  await inline.locator('.md-h1').waitFor();await inline.evaluate(el=>el.dataset.loadedMarker='keep')
  await page.evaluate(()=>renderMessages({keepScroll:true}))
  assert.equal(await inline.getAttribute('data-loaded-marker'),'keep')
  // Failed HTTP requests show a retry action and download remains available.
  const route=url=>decodeURIComponent(url.pathname)===`/${files['配置.json'].src}`;let fail=true
  await page.route(route,r=>fail?r.fulfill({status:503,body:'unavailable'}):r.continue())
  await preview('配置.json');assert.equal(await body.locator('[role=alert]').count(),1)
  fail=false;await body.locator('[data-preview-retry]').click();await body.locator('pre').waitFor();await close()
  await page.unroute(route)
  let releaseRequest, markRequested
  const gate=new Promise(resolve=>releaseRequest=resolve), requested=new Promise(resolve=>markRequested=resolve)
  await page.route(route,async r=>{markRequested();await gate;try{await r.continue()}catch{}})
  await page.locator('#messages .user-file').filter({hasText:'配置.json'}).first().locator('[data-file-preview]').click()
  await requested;assert.equal(await body.getAttribute('aria-busy'),'true')
  await page.keyboard.press('Escape')
  await preview('项目说明.md');releaseRequest()
  await page.unrouteAll({behavior:'wait'})
  assert.equal(await body.locator('.md-h1').textContent(),'项目说明');await close()
  assert.deepEqual(errors,[])
  console.log('ok   closing a loading preview cancels it; its response cannot overwrite another file')
  console.log('ok   large content truncates, unknown/corrupt/oversized files fail safely, generated Markdown stays loaded, HTTP failures can retry')
} finally {
  if(server&&running(server.process().pid))owned=[...new Set([...owned,...descendants(server.process().pid)])]
  try{await page?.close()}finally{try{await context?.close()}finally{try{await browser?.close()}finally{await server?.close()}}}
  for(const pid of owned.filter(running)){try{process.kill(pid,'SIGTERM')}catch{}}
  let remaining=[];try{await until('browser cleanup',()=>owned.every(pid=>!running(pid)),5000)}catch{remaining=owned.filter(running)}
  await app?.stop();assert.deepEqual(remaining,[],`Task-owned browser PIDs remain: ${remaining}`)
  console.log('ok   all task-owned browser sessions/processes and test service cleaned up')
}
