'use client'
import { toast } from 'sonner'
// Workspace actions already have inline error/result state. Transient notices must
// not cover the primary action or intercept clicks during the next operation.
const options={position:'top-center' as const,style:{pointerEvents:'none' as const},duration:3000}
export const workspaceToast={
  success:(message:string)=>toast.success(message,options),
  error:(message:string)=>toast.error(message,options),
  warning:(message:string)=>toast.warning(message,options),
}
