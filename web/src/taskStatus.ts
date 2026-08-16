// 任务状态映射:TaskBoard / Sidebar / AgentPanel 共用
// (色块走结构主义三原色语义:绿=完成、黄=进行、红=失败、灰=待执行)

/** 状态 → 色块/标签 class(done / active / error / pending) */
export function taskStatusClass(status: string): string {
  switch (status) {
    case 'done': return 'done';
    case 'planning':
    case 'running': return 'active';
    case 'error':
    case 'ended': return 'error';
    default: return 'pending';
  }
}

/** 状态 → 中文文案 */
export function taskStatusLabel(status: string): string {
  switch (status) {
    case 'pending': return '待执行';
    case 'planning': return '规划中';
    case 'running': return '执行中';
    case 'done': return '已完成';
    case 'error': return '出错';
    case 'ended': return '已停止';
    default: return status;
  }
}
