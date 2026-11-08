import { nativeJsonRequest } from './native-json-transport'
import { type HomeTasks } from './home-model'
import { ReviewError } from './review-logic'

export async function homeTasks(params: URLSearchParams): Promise<HomeTasks> {
  return nativeJsonRequest<HomeTasks>('v1/metadata', {
    command: 'processing-home-tasks',
    body: Object.fromEntries(params),
  }, {
    label: '全局任务状态读取',
    completion: 'required',
    error: (message, status) => new ReviewError(message, status),
  })
}
