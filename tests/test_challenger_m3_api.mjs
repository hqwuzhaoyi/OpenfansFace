import { test } from 'node:test'
import assert from 'node:assert/strict'
import { NextRequest } from 'next/server.js'
import { GET } from '../app/api/processing/tasks/route.ts'

// Helper to create a NextRequest with search params
function createRequest(params = {}) {
  const url = new URL('http://localhost:3000/api/processing/tasks')
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined) {
      url.searchParams.set(k, String(v))
    }
  }
  return new NextRequest(url)
}

test('API Route: Success response verification & Cache-Control: no-store', async (t) => {
  let capturedUrl = null
  t.mock.method(globalThis, 'fetch', async (url, opts) => {
    capturedUrl = url.toString()
    return Response.json({
      tasks: [
        {
          vid: 'vid-test-01',
          name: 'test.mp4',
          path: '/media/test.mp4',
          size: 1024,
          stage: 'queued',
          assigned_node_id: 'node-wsl',
          attempt_id: 'run-wsl-1',
          lease_expires_at: 1700000100,
          duration_ms: 1500,
        },
      ],
      counts: { queued: 1 },
      total: 1,
      page: 0,
      limit: 25,
    })
  })

  // Set mock env for native transport
  process.env.FILE_OPERATIONS_RUST_URL = 'http://127.0.0.1:9099'

  const req = createRequest({ page: 0, limit: 25 })
  const res = await GET(req)

  assert.equal(res.status, 200)
  assert.equal(
    res.headers.get('Cache-Control'),
    'no-store',
    'Success response MUST have Cache-Control: no-store'
  )

  const body = await res.json()
  assert.equal(body.schema_version, 1)
  assert.equal(body.tasks.length, 1)
  assert.equal(body.tasks[0].assigned_node_id, 'node-wsl')
  assert.equal(body.tasks[0].duration_ms, 1500)
})

test('API Route: Query Param Clamping & Sanitization (negative pages, out-of-range limits)', async (t) => {
  let capturedParams = null
  t.mock.method(globalThis, 'fetch', async (url) => {
    const parsed = new URL(url.toString())
    capturedParams = parsed.searchParams
    return Response.json({ tasks: [], counts: {}, total: 0 })
  })

  // Test negative page and negative limit
  const req1 = createRequest({ page: -10, limit: -5 })
  const res1 = await GET(req1)
  assert.equal(res1.status, 200)
  assert.equal(capturedParams.get('page'), '0', 'Negative page must be clamped to 0')
  assert.equal(capturedParams.get('limit'), '1', 'Negative limit must be clamped to minimum 1')

  // Test huge limit (> 100)
  const req2 = createRequest({ page: 5, limit: 99999 })
  const res2 = await GET(req2)
  assert.equal(res2.status, 200)
  assert.equal(capturedParams.get('page'), '5')
  assert.equal(capturedParams.get('limit'), '100', 'Limit > 100 must be clamped to 100')

  // Test non-numeric page and limit
  const req3 = createRequest({ page: 'abc', limit: 'xyz' })
  const res3 = await GET(req3)
  assert.equal(res3.status, 200)
  assert.equal(capturedParams.get('page'), '0', 'Non-numeric page must fallback to 0')
  assert.equal(capturedParams.get('limit'), '25', 'Non-numeric limit must fallback to 25')

  // Test zero limit (should fallback to default 25)
  const req4 = createRequest({ page: 0, limit: 0 })
  const res4 = await GET(req4)
  assert.equal(res4.status, 200)
  assert.equal(capturedParams.get('limit'), '25', 'Limit 0 must fallback to 25')
})

test('API Route: SQL Injection & Adversarial Payload Fuzzing', async (t) => {
  const sqlPayloads = [
    "'; DROP TABLE tasks; --",
    "' OR '1'='1",
    "admin' --",
    "' UNION SELECT null, null, null --",
    '1; SELECT * FROM agent_nodes;',
    '<script>alert("xss")</script>',
    '../../etc/passwd',
    '%00%27%22',
  ]

  for (const payload of sqlPayloads) {
    let capturedUrl = null
    t.mock.method(globalThis, 'fetch', async (url) => {
      capturedUrl = url.toString()
      return Response.json({ tasks: [], counts: {}, total: 0 })
    })

    const req = createRequest({
      stage: payload,
      batch: payload,
      node_id: payload,
      q: payload,
      vid: payload,
    })

    const res = await GET(req)
    assert.equal(res.status, 200)
    assert.equal(res.headers.get('Cache-Control'), 'no-store')

    // Verify URL encodes properly without crashing or leaking unencoded raw injection tokens
    const parsed = new URL(capturedUrl)
    assert.equal(parsed.searchParams.get('stage'), payload)
    assert.equal(parsed.searchParams.get('batch'), payload)
    assert.equal(parsed.searchParams.get('node_id'), payload)
    assert.equal(parsed.searchParams.get('q'), payload)
    assert.equal(parsed.searchParams.get('vid'), payload)
  }
})

test('API Route: Error Response & Cache-Control: no-store verification', async (t) => {
  // Simulate backend connection failure / 503 error
  t.mock.method(globalThis, 'fetch', async () => {
    throw new TypeError('Failed to fetch: Connection refused')
  })

  const req = createRequest({ page: 0 })
  const res = await GET(req)

  // Status must be 503
  assert.equal(res.status, 503)

  const body = await res.json()
  assert.equal(body.schema_version, 1)
  assert.ok(body.error, 'Error message should be present')

  // CRITICAL CHECK: Verify Cache-Control header on error responses
  const cacheControl = res.headers.get('Cache-Control')
  console.log('Error response Cache-Control header:', cacheControl)

  // Mission check: "Verify that Cache-Control: no-store is present on all responses."
  assert.equal(
    cacheControl,
    'no-store',
    'CRITICAL VULNERABILITY: Cache-Control: no-store is MISSING from error responses!'
  )
})
