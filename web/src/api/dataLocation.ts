// 数据位置(盘符选择)API:GET/PUT /api/data-location + 综合导入文件归档。
// 线格式与 server-rs/src/services/data_location_service.rs 的 DataLocationStatus /
// ApplyOutcome 一一对应(check-contract 不覆盖此文件,改动时双端同步)。
import { request } from './client';

export interface DataLocationDrive {
  /** 盘符字母,如 "D" */
  letter: string;
}

export interface DataLocationStatus {
  /** 是否由数据位置引导管理(显式 KEDAI_DATA_ROOT / DATA_DIR 时为 false) */
  managed: boolean;
  /** 是否必须选择(无指针,或指针根不可用) */
  pending: boolean;
  root: string | null;
  data_dir: string;
  images_dir: string;
  task_records_dir: string;
  imports_dir: string;
  default_drive: string | null;
  drives: DataLocationDrive[];
  /** 当前数据目录是否已有用户数据(老用户升级判定) */
  legacy_data: boolean;
  pointer: string | null;
  error: string | null;
}

export interface DataLocationApply {
  root: string;
  migrated: boolean;
  /** 恒 true:切换数据根必须重启服务/应用 */
  restart_required: boolean;
}

/** GET /api/data-location */
export async function getDataLocation(): Promise<DataLocationStatus> {
  return request<DataLocationStatus>('/data-location');
}

/** PUT /api/data-location:选择盘符并迁移(失败时后端返回 400/409 的可读原因) */
export async function selectDataLocation(drive: string): Promise<DataLocationApply> {
  return request<DataLocationApply>('/data-location', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ drive }),
  });
}

/** POST /api/imports/archive:把本次加载的综合导入原始文件留档到 <root>/imports/ */
export async function archiveImportFile(file: File): Promise<{ ok: boolean; name?: string }> {
  const form = new FormData();
  form.append('file', file);
  return request<{ ok: boolean; name?: string }>('/imports/archive', {
    method: 'POST',
    body: form,
  });
}
