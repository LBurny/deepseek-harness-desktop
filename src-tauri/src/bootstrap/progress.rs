use serde::Serialize;

/// dsh-progress 事件负载（结构化，由后端统一计算百分比，前端只展示）。
/// stage: runtime | starting | ready | stopped | error
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProgressPayload {
    pub stage: &'static str,
    pub message: String,
    pub percent: Option<u8>,
}

impl ProgressPayload {
    pub fn new(stage: &'static str, message: impl Into<String>, percent: Option<u8>) -> Self {
        Self {
            stage,
            message: message.into(),
            percent,
        }
    }
}

/// 回退部署（复制约 230MB）耗时远超其他阶段，发生时 runtime 阶段占 0..=70；
/// 原地运行的运行时准备是毫秒级，只占 0..=15。
pub const RUNTIME_SHARE_INPLACE: u8 = 15;
pub const RUNTIME_SHARE_DEPLOY: u8 = 70;
/// 等待就绪阶段前端缓动的上限：dsh 没有细分进度信号，后端只在真正就绪时报 100
pub const WAITING_CEILING: u8 = 95;

/// runtime 阶段结束后（即 starting 阶段起点）的百分比。
pub fn starting_percent(deployed: bool) -> u8 {
    if deployed {
        RUNTIME_SHARE_DEPLOY
    } else {
        RUNTIME_SHARE_INPLACE
    }
}

/// 部署复制的字节进度映射到 runtime 阶段区间 0..=70。
pub fn copy_percent(copied: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let frac = copied.min(total) as f64 / total as f64;
    (frac * RUNTIME_SHARE_DEPLOY as f64).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_percent_boundaries() {
        assert_eq!(copy_percent(0, 100), 0);
        assert_eq!(copy_percent(100, 100), RUNTIME_SHARE_DEPLOY);
        assert_eq!(copy_percent(0, 0), 0, "总量未知时不应报进度");
    }

    #[test]
    fn copy_percent_clamps_overrun() {
        assert_eq!(copy_percent(150, 100), RUNTIME_SHARE_DEPLOY);
    }

    #[test]
    fn copy_percent_is_monotonic() {
        let mut last = 0;
        for i in 0..=100u64 {
            let p = copy_percent(i * 10, 1000);
            assert!(p >= last, "percent must not decrease");
            last = p;
        }
    }

    #[test]
    fn starting_percent_depends_on_deploy() {
        assert_eq!(starting_percent(false), RUNTIME_SHARE_INPLACE);
        assert_eq!(starting_percent(true), RUNTIME_SHARE_DEPLOY);
    }

    #[test]
    fn payload_serializes() {
        let p = ProgressPayload::new("starting", "正在启动 dsh 服务…", Some(15));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["stage"], "starting");
        assert_eq!(json["percent"], 15);
        let e = ProgressPayload::new("error", "boom", None);
        assert!(serde_json::to_value(&e).unwrap()["percent"].is_null());
    }
}
