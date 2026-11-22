import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import vm from 'node:vm'
import ts from 'typescript'
import { settleBackgroundTask } from '../lib/background-operations-model.ts'

function transpileLoad(relPath, modules, extraGlobals = {}) {
  const code = ts.transpileModule(
    fs.readFileSync(new URL(relPath, import.meta.url), 'utf8'),
    {
      compilerOptions: {
        module: ts.ModuleKind.CommonJS,
        target: ts.ScriptTarget.ES2022,
        esModuleInterop: true,
      },
    },
  ).outputText
  const exports = {}
  const module = { exports }
  vm.runInNewContext(code, {
    module,
    exports,
    Buffer,
    URL,
    URLSearchParams,
    Date,
    Promise,
    Set,
    Map,
    Error,
    Number,
    String,
    Boolean,
    Array,
    Object,
    Math,
    console,
    require: (id) => {
      assert.ok(id in modules, `Unexpected import: ${id}`)
      return modules[id]
    },
    ...extraGlobals,
  })
  return module.exports
}

test('settleBackgroundTask emits synchronously under deferred React batching and merges latest fields', () => {
  const initialTask = {
    id: 'op-1',
    kind: 'person-file',
    action: 'delete-files',
    person: '菠萝啤beer',
    status: 'running',
    phase: 'queued',
    error: '',
  }
  let storedTasks = [initialTask]
  const queuedUpdaters = []
  const emitted = []

  const settled = settleBackgroundTask({
    tasks: storedTasks,
    fallback: initialTask,
    apply: (task) => ({
      ...task,
      status: 'complete',
      result: { ok: true, id: 'op-1', person: '菠萝啤beer', changed: 1, files: [] },
      error: '',
      finishedAt: 123456789,
    }),
    updateTask: (id, updater) => {
      queuedUpdaters.push(() => {
        storedTasks = storedTasks.map((t) => (t.id === id ? updater(t) : t))
      })
    },
    emit: (task) => {
      emitted.push(task)
    },
  })

  // Updater has NOT executed yet (React 19 automatic batching behavior)
  assert.equal(queuedUpdaters.length, 1)
  assert.equal(storedTasks[0].status, 'running')
  // Yet emit() already received the settled task synchronously
  assert.equal(emitted.length, 1)
  assert.equal(emitted[0], settled)
  assert.equal(emitted[0].status, 'complete')
  assert.equal(emitted[0].person, '菠萝啤beer')

  // Simulate an intervening phase update before React flushes the batch
  storedTasks = storedTasks.map((t) => ({ ...t, phase: 'ledger_committed' }))
  for (const run of queuedUpdaters) run()
  assert.equal(storedTasks[0].status, 'complete')
  assert.equal(storedTasks[0].phase, 'ledger_committed')
  assert.equal(storedTasks[0].finishedAt, 123456789)
})

test('applyPersonFileOperationAsync and status/reconcile await Redis invalidation before resolving', async () => {
  const opId = '11111111-1111-4111-8111-111111111111'
  let invalidatedCount = 0
  let releaseInvalidation = null
  let currentMode = 'sync_complete'
  const statusQueue = []

  const backend = transpileLoad('../lib/file-operation-backend.ts', {
    './native-json-transport': {
      nativeJsonRequest: async (_route, body) => {
        if (body === null) {
          return statusQueue.shift()
        }
        if (body?.mode === 'reconcile') {
          return { id: opId, status: 'complete', result: { id: opId, ok: true } }
        }
        if (currentMode === 'sync_complete') {
          return { id: opId, ok: true, person: '菠萝啤beer', changed: 1, staged: true, files: [] }
        }
        return { id: opId, status: 'running', accepted: true }
      },
    },
    'node:fs': {},
    'node:path': { join: (...args) => args.join('/') },
    'node:crypto': { randomUUID: () => opId },
    './env': { BASE: '/tmp/base' },
    './integration': {
      invalidateIntegrationCacheAsync: () => {
        invalidatedCount++
        return new Promise((resolve) => {
          releaseInvalidation = resolve
        })
      },
    },
    './file-operation-routing': {
      fileOperationOwner: () => ({ backend: 'rust', action: 'delete-files' }),
      selectedPersonFileBackend: () => 'rust',
      validateFileOperationId: () => {},
      FileOperationRoutingError: class extends Error {
        constructor(message, status = 400) {
          super(message)
          this.status = status
        }
      },
    },
  })

  // 1. Synchronous completion via async wrapper MUST await invalidateIntegrationCacheAsync()
  currentMode = 'sync_complete'
  let resolved = false
  const applyPromise = backend.applyPersonFileOperationAsync(
    { id: opId, action: 'delete-files', person: '菠萝啤beer', files: [] },
    'v1',
  ).then((res) => {
    resolved = true
    return res
  })

  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 1)
  assert.equal(resolved, false, 'must wait for invalidateIntegrationCacheAsync before resolving')
  releaseInvalidation()
  const applyResult = await applyPromise
  assert.equal(applyResult.ok, true)

  // 2. 202 accepted MUST NOT invalidate prematurely
  currentMode = 'accepted'
  const acceptedResult = await backend.applyPersonFileOperationAsync(
    { id: opId, action: 'delete-files', person: '菠萝啤beer', files: [] },
    'v1',
  )
  assert.equal(acceptedResult.accepted, true)
  assert.equal(invalidatedCount, 1)

  // 3. Status 'complete' and 'rolled_back' MUST await invalidateIntegrationCacheAsync
  statusQueue.push({ id: opId, status: 'complete', result: { id: opId, ok: true } })
  let statusDone = false
  const statusPromise = backend.personFileOperationStatus(opId).then((s) => {
    statusDone = true
    return s
  })
  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 2)
  assert.equal(statusDone, false)
  releaseInvalidation()
  await statusPromise

  statusQueue.push({ id: opId, status: 'rolled_back' })
  const rbPromise = backend.personFileOperationStatus(opId)
  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 3)
  releaseInvalidation()
  await rbPromise

  // 4. Reconcile 'complete' awaits invalidation too
  const recPromise = backend.reconcilePersonFileOperation(
    { id: opId, action: 'delete-files', person: '菠萝啤beer' },
    'v1',
  )
  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 4)
  releaseInvalidation()
  await recPromise
})

test('nativeArchiveApplyAsync and nativeArchiveProgress await invalidation when terminal result returns', async () => {
  let invalidatedCount = 0
  let releaseInvalidation = null
  let nextResponse = null

  const archiveBackend = transpileLoad('../lib/archive-native-backend.ts', {
    './native-json-transport': {
      nativeJsonRequest: async () => nextResponse,
    },
    'node:fs': {
      readFileSync: () => {
        const err = new Error('ENOENT')
        err.code = 'ENOENT'
        throw err
      },
    },
    'node:path': { join: (...args) => args.join('/') },
    './env': { BASE: '/tmp/base' },
    './file-operation-routing': { validateFileOperationId: () => {} },
    './integration': {
      invalidateIntegrationCacheAsync: () => {
        invalidatedCount++
        return new Promise((resolve) => {
          releaseInvalidation = resolve
        })
      },
    },
  })

  // Accepted/running does not invalidate
  nextResponse = { accepted: true, status: 'running' }
  await archiveBackend.nativeArchiveApplyAsync({ vids: ['v1'] })
  assert.equal(invalidatedCount, 0)

  // Fast sync completion awaits invalidation
  nextResponse = { ok: true, moved: 1, remaining: 0, errors: [] }
  let done = false
  const applyP = archiveBackend.nativeArchiveApplyAsync({ vids: ['v1'] }).then((r) => {
    done = true
    return r
  })
  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 1)
  assert.equal(done, false)
  releaseInvalidation()
  await applyP

  // nativeArchiveProgress(null) never invalidates
  nextResponse = { job: { id: 'j1', status: 'running' }, result: { ok: true } }
  await archiveBackend.nativeArchiveProgress(null)
  assert.equal(invalidatedCount, 1)

  // nativeArchiveProgress(id) with result awaits invalidation
  const opId = '22222222-2222-4222-8222-222222222222'
  let progDone = false
  const progP = archiveBackend.nativeArchiveProgress(opId).then((r) => {
    progDone = true
    return r
  })
  await new Promise((r) => setTimeout(r, 10))
  assert.equal(invalidatedCount, 2)
  assert.equal(progDone, false)
  releaseInvalidation()
  await progP
})

test('GET /api/persons/library sets Cache-Control: no-store and refresh=1 bypasses stale Redis cache', async () => {
  const redisStore = new Map([
    ['persons:library:detail:菠萝啤beer', { name: '菠萝啤beer', files: [{ vid: 'stale-1' }, { vid: 'stale-2' }] }],
    ['persons:library:list', { persons: [{ name: 'stale-person' }] }],
  ])
  const epoch = 1
  const setCalls = []

  const route = transpileLoad('../app/api/persons/library/route.ts', {
    'next/server': {
      NextRequest: class {},
      NextResponse: {
        json: (body, init = {}) => ({
          body,
          status: init.status ?? 200,
          headers: new Map(Object.entries(init.headers ?? {})),
        }),
      },
    },
    '@/lib/person-library': {
      libraryPeople: async () => [{ name: '菠萝啤beer' }],
      personLibrary: async (name) => ({ name, files: [{ vid: 'fresh-only' }], recommendations: [], folders: [], queued: 0, directory_exists: true }),
    },
    '@/lib/person-library-stats': {
      personDirectoryStats: async () => ({ stats: { '菠萝啤beer': { video_count: 1, total_bytes: 100 } } }),
    },
    '@/lib/person-names': {
      canonicalPersonName: (name) => name,
    },
    '@/lib/review-logic': {
      validateCreator: (name) => name,
    },
    '@/lib/redis': {
      cachedRedisJson: async (key, _ttl, loader) => {
        if (redisStore.has(key)) return redisStore.get(key)
        const val = await loader()
        redisStore.set(key, val)
        return val
      },
      getRedisCacheEpoch: () => epoch,
      redisSetJson: async (key, value, ttl) => {
        setCalls.push({ key, ttl })
        redisStore.set(key, value)
      },
    },
  })

  const makeReq = (url) => ({ nextUrl: new URL(url, 'http://localhost') })

  // Detail cached read still returns no-store header
  const cachedDetailRes = await route.GET(makeReq('/api/persons/library?name=%E8%8F%A0%E8%90%9D%E5%95%A4beer'))
  assert.equal(cachedDetailRes.headers.get('Cache-Control'), 'no-store')
  assert.deepEqual(cachedDetailRes.body.files, [{ vid: 'stale-1' }, { vid: 'stale-2' }])

  // Detail refresh=1 bypasses stale Redis cache and updates Redis
  const freshDetailRes = await route.GET(makeReq('/api/persons/library?name=%E8%8F%A0%E8%90%9D%E5%95%A4beer&refresh=1'))
  assert.equal(freshDetailRes.headers.get('Cache-Control'), 'no-store')
  assert.deepEqual(freshDetailRes.body.files, [{ vid: 'fresh-only' }])
  assert.deepEqual(redisStore.get('persons:library:detail:菠萝啤beer').files, [{ vid: 'fresh-only' }])
  assert.equal(setCalls[0].key, 'persons:library:detail:菠萝啤beer')

  // List cached read has no-store header
  const cachedListRes = await route.GET(makeReq('/api/persons/library'))
  assert.equal(cachedListRes.headers.get('Cache-Control'), 'no-store')
  assert.deepEqual(cachedListRes.body.persons, [{ name: 'stale-person' }])

  // List refresh=1 bypasses stale Redis list cache and updates Redis
  const freshListRes = await route.GET(makeReq('/api/persons/library?refresh=1'))
  assert.equal(freshListRes.headers.get('Cache-Control'), 'no-store')
  assert.deepEqual(freshListRes.body.persons, [{ name: '菠萝啤beer' }])
  assert.deepEqual(redisStore.get('persons:library:list').persons, [{ name: '菠萝啤beer' }])
})
