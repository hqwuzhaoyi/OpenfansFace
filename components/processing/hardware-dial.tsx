'use client'

import { memo } from 'react'
import { cn } from '@/lib/utils'

export interface HardwareDialProps {
  label: string
  type: 'ram' | 'vram' | 'gpu_util'
  usedBytes?: number | null
  totalBytes?: number | null
  percent?: number | null
  isCpuOnly?: boolean
  isOffline?: boolean
  className?: string
}

function formatGb(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '未知'
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

export const HardwareDial = memo(function HardwareDial({
  label,
  type,
  usedBytes,
  totalBytes,
  percent,
  isCpuOnly = false,
  isOffline = false,
  className = '',
}: HardwareDialProps) {
  let calculatedPercent: number | null = null
  let ratioText = ''
  let valueDisplay = '未知'
  let isNotApplicable = false
  let isUnknown = false

  if (isCpuOnly && (type === 'vram' || type === 'gpu_util')) {
    isNotApplicable = true
    valueDisplay = '不适用'
    ratioText = '纯 CPU 节点'
  } else if (isOffline) {
    isUnknown = true
    valueDisplay = '未知'
    ratioText =
      typeof totalBytes === 'number' && Number.isFinite(totalBytes) && totalBytes > 0
        ? `未知 / ${formatGb(totalBytes)}`
        : '失联未采集'
  } else if (type === 'gpu_util') {
    if (
      percent === null ||
      percent === undefined ||
      typeof percent !== 'number' ||
      !Number.isFinite(percent) ||
      percent < 0
    ) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = '未采集'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round(percent)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = 'GPU 实时负载'
    }
  } else {
    // RAM or VRAM
    const isValidTotal =
      typeof totalBytes === 'number' && Number.isFinite(totalBytes) && totalBytes > 0
    const isValidUsed =
      typeof usedBytes === 'number' && Number.isFinite(usedBytes) && usedBytes >= 0

    if (!isValidTotal || !isValidUsed) {
      isUnknown = true
      valueDisplay = '未知'
      ratioText = isValidTotal ? `未知 / ${formatGb(totalBytes)}` : '未知'
    } else {
      calculatedPercent = Math.min(100, Math.max(0, Math.round((usedBytes / totalBytes) * 100)))
      valueDisplay = `${calculatedPercent}%`
      ratioText = `${formatGb(usedBytes)} / ${formatGb(totalBytes)}`
    }
  }

  const size = 80
  const strokeWidth = 6
  const radius = (size - strokeWidth) / 2
  const circumference = 2 * Math.PI * radius
  const arcLength = (240 / 360) * circumference
  const strokeDashoffset =
    calculatedPercent !== null
      ? arcLength - (calculatedPercent / 100) * arcLength
      : arcLength

  const strokeColor =
    calculatedPercent === null
      ? 'var(--border)'
      : calculatedPercent > 90
      ? 'var(--wb-danger, #ef4444)'
      : calculatedPercent > 75
      ? 'var(--wb-warning, #f59e0b)'
      : 'var(--wb-success, #10b981)'

  return (
    <div
      className={cn('ws-hw-dial', className)}
      role="meter"
      aria-label={`${label}使用率`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={calculatedPercent ?? undefined}
      aria-valuetext={isNotApplicable ? '不适用' : isUnknown ? '未知' : `${calculatedPercent}%`}
    >
      <div className="ws-dial-svg-wrapper">
        <svg
          width={size}
          height={size}
          viewBox={`0 0 ${size} ${size}`}
          className="ws-dial-svg"
          aria-hidden="true"
        >
          {/* Background Track (240-degree arc) */}
          <circle
            cx={size / 2}
            cy={size / 2}
            r={radius}
            fill="none"
            stroke="var(--border)"
            strokeWidth={strokeWidth}
            strokeDasharray={`${arcLength} ${circumference}`}
            strokeDashoffset={0}
            strokeLinecap="round"
            transform={`rotate(150 ${size / 2} ${size / 2})`}
          />
          {/* Active Value Arc */}
          {!isNotApplicable && !isUnknown && calculatedPercent !== null && (
            <circle
              cx={size / 2}
              cy={size / 2}
              r={radius}
              fill="none"
              stroke={strokeColor}
              strokeWidth={strokeWidth}
              strokeDasharray={`${arcLength} ${circumference}`}
              strokeDashoffset={strokeDashoffset}
              strokeLinecap="round"
              transform={`rotate(150 ${size / 2} ${size / 2})`}
              className="ws-dial-value-arc"
            />
          )}
          {/* Indeterminate dashed marker if unknown */}
          {isUnknown && (
            <circle
              cx={size / 2}
              cy={size / 2}
              r={radius}
              fill="none"
              stroke="var(--border-strong, #737373)"
              strokeWidth={strokeWidth}
              strokeDasharray="4 4"
              strokeDashoffset={0}
              transform={`rotate(150 ${size / 2} ${size / 2})`}
            />
          )}
        </svg>

        {/* Center Text */}
        <div className="ws-dial-center">
          <span className="ws-dial-label">{label}</span>
          <strong
            className={cn('ws-dial-val', {
              'ws-dial-val-muted': isUnknown || isNotApplicable,
            })}
          >
            {valueDisplay}
          </strong>
        </div>
      </div>

      {/* Subtitle / Ratio */}
      <span className="ws-dial-ratio" title={ratioText}>
        {ratioText}
      </span>
    </div>
  )
})
