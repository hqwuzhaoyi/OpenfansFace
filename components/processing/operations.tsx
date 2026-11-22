'use client'

import { useCallback } from 'react'
import { Layers3, LayoutGrid, ListTodo, Server, ShieldCheck } from 'lucide-react'
import { useWorkspaceQuery } from '@/components/workbench/state'
import { WorkspaceTabs } from '@/components/workbench/controls'
import ProcessingDashboard from './dashboard'
import ProcessingTaskQueue from './task-queue'
import NodeView from './node-view'
import DeploymentView from './deployment-view'
import './operations.css'

export type OperationsTab = 'tasks' | 'nodes' | 'deploy'
export type OperationsView = 'dashboard' | 'queue'

export default function Operations({ defaultView = 'dashboard' }: { defaultView?: OperationsView } = {}) {
  const { values, update } = useWorkspaceQuery(
    {
      tab: 'tasks' as OperationsTab,
      view: '' as string,
      stage: '',
      batch: '',
      node_id: '',
      waiting_reason: '',
    },
    { tab: ['tasks', 'nodes', 'deploy'] as const }
  )

  const activeTab: OperationsTab = (values.tab as OperationsTab) || 'tasks'
  const hasQueueFilter = Boolean(values.stage || values.batch || values.node_id || values.waiting_reason)
  const activeView: OperationsView =
    values.view === 'queue' || values.view === 'dashboard'
      ? (values.view as OperationsView)
      : hasQueueFilter
        ? 'queue'
        : defaultView

  const navigateOverview = useCallback(
    () =>
      update({
        tab: 'tasks',
        view: 'dashboard',
        stage: '',
        batch: '',
        node_id: '',
        waiting_reason: '',
      }),
    [update]
  )

  return (
    <div className="ws-operations-root">
      <div className="ws-view-tabs ws-operations-tabs" role="navigation" aria-label="操作台三视图">
        <WorkspaceTabs
          value={activeTab}
          onChange={(tab) => update({ tab: tab as OperationsTab })}
          label="操作台视图"
          className="ws-operations-primary-tabs"
          listClassName="ws-operations-primary-list"
          items={[
            { value: 'tasks', label: '任务视图', icon: <Layers3 size={14} /> },
            { value: 'nodes', label: '节点视图', icon: <Server size={14} /> },
            { value: 'deploy', label: '部署视图', icon: <ShieldCheck size={14} /> },
          ]}
        />
        {activeTab === 'tasks' && (
          <WorkspaceTabs
            value={activeView}
            onChange={(view) => update({ view: view as OperationsView })}
            label="任务子视图"
            variant="segment"
            className="ws-subview-toggle"
            listClassName="ws-subview-tablist"
            items={[
              { value: 'dashboard', label: '批次总览', icon: <LayoutGrid size={13} /> },
              { value: 'queue', label: '任务队列', icon: <ListTodo size={13} /> },
            ]}
          />
        )}
      </div>

      <div className="ws-operations-view-container">
        {activeTab === 'tasks' && (
          activeView === 'queue' ? (
            <ProcessingTaskQueue onNavigateOverview={navigateOverview} />
          ) : (
            <ProcessingDashboard onNavigateOverview={navigateOverview} />
          )
        )}
        {activeTab === 'nodes' && <NodeView onNavigateOverview={navigateOverview} />}
        {activeTab === 'deploy' && <DeploymentView onNavigateOverview={navigateOverview} />}
      </div>
    </div>
  )
}
