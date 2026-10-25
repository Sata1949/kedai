// 音频播放器域类型(阶段五 5a;契约对齐酒馆助手 audio.d.ts)。

/** 音频曲目(仅 URL 播放,本地文件上传留扩展位) */
export interface AudioTrack {
  /** 标题 */
  title: string;
  /** 音频的网络链接 */
  url: string;
}

/** 播放模式(与酒馆助手 AudioSettings.mode 对齐) */
export type AudioMode = 'repeat_one' | 'repeat_all' | 'shuffle' | 'play_one_and_stop';

/** 单通道设置(部分字段合并更新时全部可选) */
export interface AudioChannelSettings {
  /** 是否启用 */
  enabled: boolean;
  /** 当前播放模式 */
  mode: AudioMode;
  /** 是否静音 */
  muted: boolean;
  /** 当前音量 (0-100) */
  volume: number;
}

/** 音频通道标识 */
export type AudioChannelType = 'bgm' | 'ambient';

/** 双通道全量状态 */
export interface AudioState {
  bgm: AudioChannelState;
  ambient: AudioChannelState;
}

export interface AudioChannelState extends AudioChannelSettings {
  playlist: AudioTrack[];
}

/** 沙箱内 getCurrentAudio 返回(当前选中/播放的曲目信息) */
export interface CurrentAudio {
  /** 当前选中/正在播放的音频链接,未选中曲目时为空字符串 */
  src: string;
  /** 当前选中音频的标题,未匹配到时为空字符串 */
  title: string;
  /** 是否正在播放 */
  playing: boolean;
  /** 播放进度 (0-100) */
  progress: number;
}
