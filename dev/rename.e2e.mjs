#!/usr/bin/env node
// Upgrade compatibility, using only isolated homes and in-memory browser storage.
import assert from 'node:assert/strict'
import {spawnSync} from 'node:child_process'
import {mkdtempSync,mkdirSync,existsSync,readFileSync,writeFileSync,rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {runInNewContext} from 'node:vm'
import {root} from './harness.mjs'

const binary=process.env.DOTBOT_TEST_BINARY||join(root,'target/debug/dotbot')
const directory=mkdtempSync(join(tmpdir(),'dotbot-rename-'))
const env={...process.env,HOME:directory}
for(const name of Object.keys(env))if(/^(DOTBOT|TEAMBOT)_/.test(name))delete env[name]
const run=(args,extra={},input)=>{
  const result=spawnSync(binary,args,{env:{...env,...extra},input,encoding:'utf8',timeout:15000})
  assert.equal(result.status,0,result.stderr)
  return result.stdout
}
try{
  assert.match(run(['--version']),/^dotbot /)
  assert.match(run(['--help']),/Usage: dotbot/)
  run(['skill','list'])
  assert.ok(existsSync(join(directory,'.dotbot/plugin')))
  rmSync(join(directory,'.dotbot'),{recursive:true})
  mkdirSync(join(directory,'.teambot'))
  writeFileSync(join(directory,'.teambot/state.json'),'existing team')
  run(['skill','list'])
  assert.ok(existsSync(join(directory,'.teambot/plugin')))
  assert.equal(existsSync(join(directory,'.dotbot')),false)
  assert.equal(readFileSync(join(directory,'.teambot/state.json'),'utf8'),'existing team')
  const legacy=join(directory,'legacy-env'),current=join(directory,'current-env'),explicit=join(directory,'explicit')
  run(['skill','list'],{TEAMBOT_HOME:legacy})
  assert.ok(existsSync(join(legacy,'plugin')))
  run(['skill','list'],{TEAMBOT_HOME:legacy,DOTBOT_HOME:current})
  assert.ok(existsSync(join(current,'plugin')))
  run(['--home',explicit,'skill','list'],{TEAMBOT_HOME:legacy,DOTBOT_HOME:current})
  assert.ok(existsSync(join(explicit,'plugin')))
  const request=JSON.stringify({jsonrpc:'2.0',id:1,method:'tools/list'})+'\n'
  const oldEnv={TEAMBOT_URL:'http://127.0.0.1:1',TEAMBOT_TOKEN:'fixture',TEAMBOT_CONVERSATION:'bot:fixture',TEAMBOT_LOCAL:'1'}
  const legacyTools=JSON.parse(run(['mcp'],oldEnv,request)).result.tools
  assert.ok(legacyTools.some(tool=>tool.name==='local_run'))
  const newTools=JSON.parse(run(['mcp'],{...oldEnv,DOTBOT_LOCAL:'0'},request)).result.tools
  assert.ok(!newTools.some(tool=>tool.name==='local_run'))
  const storage=initial=>{
    const values=new Map(Object.entries(initial))
    return {getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)}
  }
  const localStorage=storage({'teambot.theme':'dark','teambot.lang':'zh-CN'})
  const sessionStorage=storage({'teambot.upgrade-drafts':'unsent draft and attachment metadata'})
  const document={documentElement:{dataset:{}}}
  const theme=readFileSync(join(root,'web/theme.js'),'utf8')
  runInNewContext(theme,{localStorage,sessionStorage,document})
  assert.equal(document.documentElement.dataset.theme,'dark')
  assert.equal(localStorage.getItem('dotbot.lang'),'zh-CN')
  assert.equal(sessionStorage.getItem('dotbot.upgrade-drafts'),'unsent draft and attachment metadata')
  assert.equal(localStorage.getItem('teambot.theme'),null)
  localStorage.setItem('dotbot.theme','light');localStorage.setItem('teambot.theme','dark')
  runInNewContext(theme,{localStorage,sessionStorage,document})
  assert.equal(document.documentElement.dataset.theme,'light')
  console.log('ok   dotbot CLI; new and legacy homes; environment aliases and precedence; MCP capabilities; preferences and unsent draft migration')
}finally{rmSync(directory,{recursive:true,force:true})}
