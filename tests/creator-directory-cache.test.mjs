import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import vm from 'node:vm'
import ts from 'typescript'
import * as model from '../lib/creator-directory-model.ts'

// Isolate filesystem and network, including each module's single-flight/backoff state.
const code = ts.transpileModule(fs.readFileSync(new URL('../lib/creator-directory.ts', import.meta.url), 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, esModuleInterop: true },
}).outputText
const rows = [{ id: '123', screen_name: 'sample_01', name: '示例', description: '' }]
function harness({ saved = null, fetcher = async () => Response.json(rows) } = {}) {
  let calls = 0, writes = 0
  const exports = {}
  const modules = {
    'node:fs/promises': { mkdir: async () => {} }, 'node:path': path,
    './env': { BASE: '/isolated-fixture' }, './creator-directory-model': model,
    './jsonio': { readJson: () => saved, writeJson: (_path, value) => { saved = value; writes++ } },
  }
  vm.runInNewContext(code, { exports, Buffer, Date, AbortSignal,
    require: id => { assert.ok(id in modules, id); return modules[id] },
    fetch: async (url, options) => {
      assert.equal(url, model.CREATOR_DIRECTORY_DATA)
      assert.equal(options.cache, 'no-store')
      assert.ok(options.signal instanceof AbortSignal)
      calls++; return fetcher()
    },
  })
  return { load: exports.creatorDirectory, stats: () => ({ calls, writes, saved }) }
}

test('concurrent lookups share one request, then reuse the persisted validated snapshot', async () => {
  const h = harness()
  const results = await Promise.all(Array.from({ length: 10 }, () => h.load()))
  assert.ok(results.every(r => r.profiles[0].handle === 'sample_01' && !r.stale))
  await h.load()
  assert.equal(h.stats().calls, 1)
  assert.equal(h.stats().writes, 1)
  const restarted = harness({ saved: h.stats().saved, fetcher: async () => { throw Error('offline') } })
  assert.equal((await restarted.load()).stale, false)
  assert.equal(restarted.stats().calls, 0)
})

test('failed refresh preserves stale data, reports it and backs off instead of hammering the source', async () => {
  const saved = { fetched_at: '2020-01-01T00:00:00Z', data: rows }
  for (const fetcher of [async () => { throw Error('offline') }, async () => Response.json({ error: 'invalid' }), async () => new Response('', { status: 503 }), async () => Response.json([])]) {
    const h = harness({ saved, fetcher })
    const result = await h.load()
    assert.equal(result.stale, true)
    assert.ok(result.warning.includes('缓存'))
    assert.equal(result.fetched_at, saved.fetched_at)
    await h.load()
    assert.equal(h.stats().writes, 0)
    assert.equal(h.stats().calls, 1)
    assert.equal(h.stats().saved, saved)
  }
})

test('cold-start failure is an error, not an empty successful match; oversized data is rejected', async () => {
  for (const fetcher of [async () => { throw Error('offline') }, async () => new Response('[]', { headers: { 'content-length': String(11 * 1024 * 1024) } }), async () => new Response('x'.repeat(11 * 1024 * 1024))]) {
    const h = harness({ fetcher })
    await assert.rejects(h.load(), /无法更新/)
    await assert.rejects(h.load(), /无法更新/)
    assert.equal(h.stats().writes, 0)
    assert.equal(h.stats().calls, 1)
  }
})
