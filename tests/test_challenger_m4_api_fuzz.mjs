import { test } from 'node:test'
import assert from 'node:assert/strict'
import { NextRequest } from 'next/server.js'
import { GET } from '../app/api/processing/tasks/route.ts'

process.env.FILE_OPERATIONS_RUST_URL = 'http://127.0.0.1:9099'

function createRequest(params = {}) {
  const url = new URL('http://localhost:3000/api/processing/tasks')
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined) {
      url.searchParams.set(k, String(v))
    }
  }
  return new NextRequest(url)
}

test('M4 API Fuzzing: Adversarial query params, clamping, and Cache-Control: no-store', async (t) => {
  let capturedBackendUrl = ''
  t.mock.method(globalThis, 'fetch', async (url) => {
    capturedBackendUrl = url.toString()
    return Response.json({
      tasks: [],
      counts: {},
      total: 0,
    })
  })

  // Negative numbers
  const req1 = createRequest({ page: -999, limit: -50 })
  const res1 = await GET(req1)
  assert.equal(res1.status, 200)
  assert.equal(res1.headers.get('Cache-Control'), 'no-store')
  const p1 = new URL(capturedBackendUrl).searchParams
  assert.equal(p1.get('page'), '0', 'Negative page must be clamped to 0')
  assert.equal(p1.get('limit'), '1', 'Negative limit must be clamped to at least 1')

  // Massive limits
  const req2 = createRequest({ page: 10, limit: 1000000 })
  const res2 = await GET(req2)
  assert.equal(res2.status, 200)
  assert.equal(res2.headers.get('Cache-Control'), 'no-store')
  const p2 = new URL(capturedBackendUrl).searchParams
  assert.equal(p2.get('page'), '10')
  assert.equal(p2.get('limit'), '100', 'Massive limit must be clamped to 100')

  // Non-numeric strings
  const req3 = createRequest({ page: 'not_a_number', limit: 'definitely_not_a_number' })
  const res3 = await GET(req3)
  assert.equal(res3.status, 200)
  assert.equal(res3.headers.get('Cache-Control'), 'no-store')
  const p3 = new URL(capturedBackendUrl).searchParams
  assert.equal(p3.get('page'), '0', 'Non-numeric page must fallback to 0')
  assert.equal(p3.get('limit'), '25', 'Non-numeric limit must fallback to 25')

  // SQL Injection vectors
  const sqlPayloads = [
    "' OR '1'='1",
    "'; DROP TABLE tasks; --",
    "' UNION SELECT null, null, null --",
    "1; EXEC xp_cmdshell('dir'); --",
    "admin' --",
    "\" or \"\"=\"",
    "' OR 1=1 #",
  ]

  for (const sqli of sqlPayloads) {
    const reqSqli = createRequest({
      q: sqli,
      stage: sqli,
      batch: sqli,
      node_id: sqli,
      waiting_reason: sqli,
      vid: sqli,
    })
    const resSqli = await GET(reqSqli)
    assert.equal(resSqli.status, 200, `SQLi payload ${sqli} should not crash the route`)
    assert.equal(resSqli.headers.get('Cache-Control'), 'no-store')
    const pSqli = new URL(capturedBackendUrl).searchParams
    assert.equal(pSqli.get('q'), sqli)
    assert.equal(pSqli.get('stage'), sqli)
    assert.equal(pSqli.get('batch'), sqli)
    assert.equal(pSqli.get('node_id'), sqli)
  }
})

test('M4 API Fuzzing: Error response verification with 503 & Cache-Control: no-store', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => {
    throw new TypeError('Backend 503 Service Unavailable / Connection refused')
  })

  const req = createRequest({ page: 0 })
  const res = await GET(req)

  assert.equal(res.status, 503)
  assert.equal(
    res.headers.get('Cache-Control'),
    'no-store',
    'Error response MUST carry Cache-Control: no-store'
  )

  const body = await res.json()
  assert.equal(body.schema_version, 1)
  assert.ok(typeof body.error === 'string' && body.error.length > 0)
})

test('M4 API Fuzzing: Pass-through of M4 fields (creator, face_skipped, ocr_result, error_type) without leaking undefined/NaN', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => {
    return Response.json({
      tasks: [
        {
          vid: 'vid-m4-01',
          name: 'matched_creator.mp4',
          path: '/media/matched_creator.mp4',
          stage: 'done',
          creator: '@alice',
          face_skipped: true,
          ocr_result: 'text_found',
          error_type: null,
          data: {
            creator: '@alice',
            face_skipped: true,
          },
        },
        {
          vid: 'vid-m4-02',
          name: 'no_text.mp4',
          path: '/media/no_text.mp4',
          stage: 'detect',
          creator: null,
          face_skipped: false,
          ocr_result: 'no_text_found',
          error_type: null,
          data: {
            face_skipped: false,
            ocr_result: 'no_text_found',
          },
        },
        {
          vid: 'vid-m4-03',
          name: 'parse_err.mp4',
          path: '/media/parse_err.mp4',
          stage: 'retry',
          creator: null,
          face_skipped: null,
          ocr_result: null,
          error_type: 'parse_failure',
          data: {
            error_type: 'parse_failure',
          },
        },
        {
          vid: 'vid-m4-04',
          name: 'missing_fields.mp4',
          path: '/media/missing_fields.mp4',
          // completely missing all fields
        },
      ],
      counts: { done: 1, detect: 1, retry: 1, queued: 1 },
      total: 4,
      page: 0,
      limit: 25,
    })
  })

  const req = createRequest({ page: 0 })
  const res = await GET(req)
  assert.equal(res.status, 200)

  const json = await res.json()
  assert.equal(json.tasks.length, 4)

  // Task 1: creator match
  const t1 = json.tasks[0]
  assert.equal(t1.creator, '@alice')
  assert.equal(t1.face_skipped, true)
  assert.equal(t1.ocr_result, 'text_found')
  assert.equal(t1.error_type, null)

  // Task 2: no text found
  const t2 = json.tasks[1]
  assert.equal(t2.creator, null)
  assert.equal(t2.face_skipped, false)
  assert.equal(t2.ocr_result, 'no_text_found')
  assert.equal(t2.error_type, null)

  // Task 3: parse failure
  const t3 = json.tasks[2]
  assert.equal(t3.creator, null)
  assert.equal(t3.face_skipped, null)
  assert.equal(t3.error_type, 'parse_failure')

  // Task 4: missing fields
  const t4 = json.tasks[3]
  assert.equal(t4.creator, null)
  assert.equal(t4.face_skipped, null)
  assert.equal(t4.ocr_result, null)
  assert.equal(t4.error_type, null)
  assert.notEqual(t4.creator, undefined)
  assert.notEqual(t4.face_skipped, undefined)

  // Ensure JSON text does not contain "undefined" or "NaN"
  const rawText = JSON.stringify(json)
  assert.ok(!rawText.includes('undefined'), 'JSON response must not contain "undefined"')
  assert.ok(!rawText.includes('NaN'), 'JSON response must not contain "NaN"')
})
