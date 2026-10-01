//! minimal 预设的上游签名探测（只读）。
//!
//! 历史：dsh rc ≤0.1.0-rc.7 的 minimal（极简模式）预设无条件挂载 PTY 持久
//! bash，而 dsh-subprocess-local 的终端检查器只实现了 linux/darwin——win32 上
//! 每次 bash 调用都以 "terminal inspection is unsupported on platform win32"
//! 失败。本模块曾是启动期原地改写补丁（tool-pwsh 变体，签名门控+marker 幂等）。
//!
//! 0.1.0-rc.8 上游自修：预设的 bash/pwsh 两行按 process.platform 互斥禁用，
//! subprocess-local 新增 win32 检查器（koffi 版 windows-inspector）。补丁器
//! 随之退役；保留签名判定是因为 tests/upstream_contract.rs 靠它当回归哨兵——
//! 上游若回退 win32 修复（状态回到 NeedsPatch），契约套件翻红，从 git 历史
//! 找回补丁器。
//!
//! 预设落点变迁：0.1.2 起在独立的 dsh-agent-presets 包（presets/minimal/
//! agent.cordis.yml）；0.2.0 起该包消失，改为 dsh-web-app/presets/minimal.patch.yml
//! 的 cordis patch insert 行（upstream::PRESET_DIR_SEGMENTS/COMPOSITION_FILE
//! 已跟随）。签名判定逻辑不变：minimal.patch.yml 仍含带 win32 门控的持久
//! bash 引用，UpstreamHandled 语义保持。

use std::fs;
use std::path::Path;

/// 历史补丁标记（曾写入文件首行注释）。只用于识别 ≤0.1.21 壳补丁过的旧
/// 运行时文件（%LOCALAPPDATA% 部署副本在 .version 比对前可能仍在）。
const MARKER: &str = "dshdesktop: win32 pwsh variant v1";

/// minimal 预设的签名状态（只读，不改写）。
/// tests/upstream_contract.rs 靠它回答"上游这版还需不需要我方补丁"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureState {
    /// 含破损签名且无平台分支：上游回退了 win32 修复，需要恢复补丁器。
    NeedsPatch,
    /// 历史 marker 已在：这份运行时被 ≤0.1.21 的壳补丁过。
    AlreadyPatched,
    /// 上游已含平台分支或形态变化：无需我方介入（rc.8 起的期望状态）。
    UpstreamHandled,
    /// 预设文件缺失（fixture 运行时等布局）。
    Missing,
}

pub fn preset_signature_state(preset_dir: &Path) -> SignatureState {
    let composition = preset_dir.join(crate::dsh::upstream::PRESET_COMPOSITION_FILE);
    let Ok(content) = fs::read_to_string(&composition) else {
        return SignatureState::Missing;
    };
    if content.contains(MARKER) {
        return SignatureState::AlreadyPatched;
    }
    if !content.contains(crate::dsh::upstream::PRESET_BROKEN_NEEDLE)
        || content.contains(crate::dsh::upstream::PRESET_PLATFORM_NEEDLE)
    {
        return SignatureState::UpstreamHandled;
    }
    SignatureState::NeedsPatch
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上游 rc.6 形态的 minimal 预设（节选关键行，签名与平台判定只看特征串）。
    const UPSTREAM_COMPOSITION_RC6: &str = r#"# The `minimal` agent preset
- id: persistent-shell
  name: cordis:group
  config:
    - id: persistent-bash
      name: '@deepseek-ai/dsh-tool-bash-persistent'
"#;

    #[test]
    fn signature_state_classification() {
        // 组合文件名取 upstream 常量（0.1.2 前是 agent.cordis.yml，0.2.0 起是
        // minimal.patch.yml——测试跟随常量，落点再搬家不用改这里）。
        let composition = crate::dsh::upstream::PRESET_COMPOSITION_FILE;
        // rc.6 形态：破损签名 + 无平台分支 → NeedsPatch
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(composition), UPSTREAM_COMPOSITION_RC6).unwrap();
        assert_eq!(preset_signature_state(dir.path()), SignatureState::NeedsPatch);

        // rc.8 形态：仍引用持久 bash，但带 win32 平台门控 → UpstreamHandled
        let fixed = tempfile::tempdir().unwrap();
        fs::write(
            fixed.path().join(composition),
            UPSTREAM_COMPOSITION_RC6.replace(
                "name: '@deepseek-ai/dsh-tool-bash-persistent'",
                "name: '@deepseek-ai/dsh-tool-bash-persistent'\n      disabled: !!js process.platform === 'win32'",
            ),
        )
        .unwrap();
        assert_eq!(
            preset_signature_state(fixed.path()),
            SignatureState::UpstreamHandled
        );

        // 历史补丁文件（marker 在）仍正确分类 → AlreadyPatched
        let patched = tempfile::tempdir().unwrap();
        fs::write(
            patched.path().join(composition),
            format!("# {MARKER}\n{UPSTREAM_COMPOSITION_RC6}"),
        )
        .unwrap();
        assert_eq!(
            preset_signature_state(patched.path()),
            SignatureState::AlreadyPatched
        );

        // 目录缺失 → Missing
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(preset_signature_state(empty.path()), SignatureState::Missing);
    }
}
