import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'

const root = process.cwd()

test('DOM & A11y: CSS Mobile Touch Targets >= 44px', () => {
  const css = fs.readFileSync(path.join(root, 'components/workbench/workspace.css'), 'utf-8')
  
  // Verify @media(max-width:767px) exists
  const startIdx = css.indexOf('@media(max-width:767px){')
  assert.ok(startIdx !== -1, 'Mobile media query must exist')

  // Find matching closing brace
  let depth = 0
  let endIdx = -1
  for (let i = startIdx; i < css.length; i++) {
    if (css[i] === '{') depth++
    else if (css[i] === '}') {
      depth--
      if (depth === 0) {
        endIdx = i
        break
      }
    }
  }
  assert.ok(endIdx !== -1, 'Matching closing brace for media query must be found')
  const mobileCss = css.slice(startIdx, endIdx + 1)

  // Verify .ws-icon touch target (used by pagination ChevronLeft, ChevronRight, refresh)
  assert.match(
    mobileCss,
    /\.ws-icon\{[^}]*min-width:\s*44px[^}]*min-height:\s*44px/,
    '.ws-icon must enforce at least 44px touch target on mobile'
  )

  // Verify search toolbar button (filter trigger InputGroupButton)
  assert.match(
    mobileCss,
    /\.ws-search\s*\[data-slot=button\]\{[^}]*min-width:\s*44px[^}]*min-height:\s*44px/,
    '.ws-search [data-slot=button] (filter button) must enforce 44px touch target on mobile'
  )

  // Verify pagination container
  assert.match(
    mobileCss,
    /\.ws-pagination\{[^}]*min-height:\s*56px/,
    '.ws-pagination must enforce >= 44px min-height'
  )

  // Verify detail actions
  assert.match(
    mobileCss,
    /\.ws-detail-actions\s*\[data-slot=button\]\{[^}]*min-height:\s*44px/,
    '.ws-detail-actions [data-slot=button] must enforce 44px min-height'
  )
})

test('DOM & A11y: Dialog Close Trigger Touch Target >= 44px', () => {
  const dialogSrc = fs.readFileSync(path.join(root, 'components/ui/dialog.tsx'), 'utf-8')

  // Verify size-11 (11 * 4px = 44px) on DialogPrimitive.Close
  assert.ok(
    dialogSrc.includes('size-11'),
    'Dialog close trigger must have Tailwind size-11 (44px x 44px touch target)'
  )
  assert.ok(
    dialogSrc.includes('<span className=\'sr-only\'>Close</span>'),
    'Dialog close trigger must include sr-only text for screen readers'
  )
})

test('DOM & A11y: TaskQueue Mobile Drawer Focus Trapping & Restoration', () => {
  const taskQueueSrc = fs.readFileSync(path.join(root, 'components/processing/task-queue.tsx'), 'utf-8')

  // Check returnFocus ref is defined
  assert.ok(
    taskQueueSrc.includes('const returnFocus = useRef<HTMLElement | null>(null)'),
    'TaskQueue must define returnFocus ref'
  )

  // Check returnFocus is recorded on inspect
  assert.ok(
    taskQueueSrc.includes('returnFocus.current = document.activeElement as HTMLElement'),
    'inspect() must capture document.activeElement before opening drawer'
  )

  // Check ResponsiveDialogContent with onCloseAutoFocus
  assert.ok(
    taskQueueSrc.includes('<ResponsiveDialogContent'),
    'Mobile drawer must use ResponsiveDialogContent'
  )
  assert.ok(
    taskQueueSrc.includes('onCloseAutoFocus={(e) => {'),
    'ResponsiveDialogContent must bind onCloseAutoFocus'
  )
  assert.ok(
    taskQueueSrc.includes('if (returnFocus.current?.isConnected) {'),
    'onCloseAutoFocus must verify returnFocus element is still connected'
  )
  assert.ok(
    taskQueueSrc.includes('returnFocus.current.focus()'),
    'onCloseAutoFocus must restore focus to trigger'
  )

  // Check screen reader accessible titles on Dialog
  assert.ok(
    taskQueueSrc.includes('<DialogTitle className="sr-only">任务详情</DialogTitle>'),
    'Dialog must have accessible DialogTitle'
  )
  assert.ok(
    taskQueueSrc.includes('<DialogDescription className="sr-only">关闭后返回当前队列位置。</DialogDescription>'),
    'Dialog must have accessible DialogDescription'
  )
})
