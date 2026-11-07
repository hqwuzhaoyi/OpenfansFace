import { test } from 'node:test'
import assert from 'node:assert/strict'
import vm from 'node:vm'
import { privacyInitScript, PRIVACY_STORAGE_KEY } from '../lib/privacy.ts'

function firstPaint(storage) {
  const document = { documentElement: { dataset: { privacy: 'on' } } }
  vm.runInNewContext(privacyInitScript, { document, localStorage: storage })
  return document.documentElement.dataset.privacy
}

test('restores privacy before hydration without requiring React or network', () => {
  assert.equal(firstPaint({ getItem(key) { assert.equal(key, PRIVACY_STORAGE_KEY); return 'on' } }), 'on')
  assert.equal(firstPaint({ getItem() { return 'off' } }), 'off')
})

test('first use keeps normal viewing, unavailable storage conceals media', () => {
  assert.equal(firstPaint({ getItem() { return null } }), 'off')
  assert.equal(firstPaint({ getItem() { throw new Error('Storage access denied') } }), 'on')
  assert.equal(firstPaint(undefined), 'on')
})
