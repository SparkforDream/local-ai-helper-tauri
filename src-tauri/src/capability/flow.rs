// ============================================================
// capability/flow.rs —— 四步编排
//
// 四步公式里的每一步单独都能跑，但用户要的是「一句话把事办完」。
// 这一层负责把它们串起来，并在岔路口做判断：
//   已经装了？→ 不装，直接告诉他装在哪
//   下载失败？→ 停，说清原因
//   需要用户点？→ 打开窗口，等他说好了
//
// 设计纪律：
//   1. 状态机，不是线性脚本。每一步都可能因为「已满足」而跳过。
//   2. 每个阶段都有明确的、对人说的话。不是错误码。
//   3. 编排层不做任何实际操作，只调 capability。
// ============================================================

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 流程当前停在哪个阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// 刚开始，需要先查有没有装过
    Checking,
    /// 已经装了，流程结束
    AlreadyInstalled,
    /// 需要下载
    NeedDownload,
    /// 已下载，待校验
    Downloaded,
    /// 校验通过，等用户同意安装
    NeedConsent,
    /// 安装程序已打开，等用户操作
    AwaitingUser,
    /// 安装完成
    Done,
    /// 出错了，停止
    Failed,
}

impl Stage {
    /// 用户在这个阶段有没有动作可做。
    /// NeedDownload 也要算进来：下载是个耗时动作，得用户点头才启动，
    /// 不能自己闷头下（万一他只想问一句、并不想装）。
    pub fn needs_user_action(self) -> bool {
        matches!(
            self,
            Stage::NeedDownload | Stage::NeedConsent | Stage::AwaitingUser
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Stage::AlreadyInstalled | Stage::Done | Stage::Failed
        )
    }
}

/// 一次流程的状态。前端持有它，每次调用把新状态传回来。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowState {
    pub recipe: String,
    pub stage: Stage,
    /// 给用户看的话。每一句都要能直接显示。
    pub say: String,
    /// 用户接下来能做什么
    pub actions: Vec<Action>,
    /// 过程证据，用于事后核对
    pub facts: Value,
}

/// 用户可选的动作。
///
/// 字段用 String 而不是 &'static str：这个结构要能被前端传回来，
/// 必须实现 Deserialize，而 &'static str 做不到。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// 动作 id，前端拿它调 advance
    pub id: String,
    /// 按钮文字
    pub label: String,
}

fn act(id: &'static str, label: &'static str) -> Action {
    Action { id: id.to_string(), label: label.to_string() }
}

// ============================================================
// 主入口
// ============================================================

/// 开始一个新流程。永远从「检查是否已装」起步。
pub fn start(recipe_id: &str) -> FlowState {
    let Some(r) = super::recipe::find(recipe_id) else {
        return FlowState {
            recipe: recipe_id.to_string(),
            stage: Stage::Failed,
            say: format!("我没有 {recipe_id} 的安装配方。"),
            actions: vec![],
            facts: json!({}),
        };
    };

    // 第一件事永远是看有没有装过，而不是直接下载。
    // 用户说「帮我装 Steam」时，有一半情况他其实已经装了，只是不知道在哪。
    let probe = super::detect::verify_install(recipe_id);
    let installed = probe
        .get("installed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if installed {
        let dir = probe
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or("未知位置");
        let complete = probe
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let say = if complete {
            format!("{} 已经装好了，在 {}。不用再装。", r.name, dir)
        } else {
            format!(
                "{} 已经装了，在 {}，但有文件缺失。\n可能装坏了，建议修复安装。",
                r.name, dir
            )
        };

        return FlowState {
            recipe: recipe_id.to_string(),
            stage: if complete {
                Stage::AlreadyInstalled
            } else {
                Stage::NeedConsent
            },
            say,
            actions: if complete {
                vec![]
            } else {
                vec![act("reinstall", "重新下载安装")]
            },
            facts: probe,
        };
    }

    // 没装过，可以开始下载
    FlowState {
        recipe: recipe_id.to_string(),
        stage: Stage::NeedDownload,
        say: format!(
            "{} 还没装。安装包 {:.1}MB，我现在下。\n{}",
            r.name,
            r.size as f64 / 1_048_576.0,
            r.note
        ),
        actions: vec![act("download", "开始下载")],
        facts: json!({ "sizeBytes": r.size }),
    }
}

/// 推进一步。根据当前 stage 决定做什么。
pub fn advance(state: &FlowState, action: &str) -> FlowState {
    let Some(r) = super::recipe::find(&state.recipe) else {
        return fail(&state.recipe, "配方丢失");
    };

    match (state.stage, action) {
        // 用户点了下载
        (Stage::NeedDownload, "download") => {
            let a = super::Args::new(json!({ "id": state.recipe }));
            match super::recipe::step1_download(&a) {
                Ok(_) => {
                    // 下载完立刻校验，不等用户催
                    let v = super::recipe::step2_verify(&a).unwrap_or(json!({}));
                    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);

                    if ok {
                        FlowState {
                            recipe: state.recipe.clone(),
                            stage: Stage::NeedConsent,
                            say: format!(
                                "下好了，校验通过。\n现在运行安装程序吗？\n\n{}",
                                r.note
                            ),
                            actions: vec![
                                act("install", "现在装"),
                                act("cancel", "先不装"),
                            ],
                            facts: v,
                        }
                    } else {
                        let detail = v
                            .get("detail")
                            .and_then(|x| x.as_str())
                            .unwrap_or("校验未通过");
                        FlowState {
                            recipe: state.recipe.clone(),
                            stage: Stage::Failed,
                            say: format!("安装包下坏了：{detail}\n要不要重新下一遍？"),
                            actions: vec![act("download", "重新下载")],
                            facts: v,
                        }
                    }
                }
                Err(e) => fail(&state.recipe, &e),
            }
        }

        // 用户同意安装
        (Stage::NeedConsent, "install") | (Stage::NeedConsent, "reinstall") => {
            if action == "reinstall" {
                // 重装要先下载
                return advance(&FlowState { stage: Stage::NeedDownload, ..state.clone() }, "download");
            }

            let a = super::Args::new(json!({ "id": state.recipe }));
            match super::recipe::step3_install(&a) {
                Ok(v) => {
                    let need = v
                        .get("needUserAction")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(true);
                    let msg = v
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("安装程序已打开。");

                    FlowState {
                        recipe: state.recipe.clone(),
                        stage: Stage::AwaitingUser,
                        say: format!("{msg}\n\n装完跟我说一声，我检查一下。"),
                        actions: vec![
                            act("check", "装好了，帮我看看"),
                            act("cancel", "算了"),
                        ],
                        facts: v,
                    }
                    .with_pending(need)
                }
                Err(e) => fail(&state.recipe, &e),
            }
        }

        // 用户说装完了，核对
        (Stage::AwaitingUser, "check") => {
            let v = super::detect::verify_install(&state.recipe);
            let installed = v
                .get("installed")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let complete = v
                .get("complete")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);

            if installed && complete {
                let dir = v.get("dir").and_then(|x| x.as_str()).unwrap_or("");
                let proc_running = v
                    .get("processRunning")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                if proc_running {
                    FlowState {
                        recipe: state.recipe.clone(),
                        stage: Stage::Done,
                        say: format!(
                            "装好了，在 {dir}，而且已经检测到主进程在运行（界面应该已经打开了）。"
                        ),
                        actions: vec![],
                        facts: v,
                    }
                } else {
                    FlowState {
                        recipe: state.recipe.clone(),
                        stage: Stage::AwaitingUser,
                        say: format!(
                            "装好了，在 {dir}，文件完整。目前还没检测到它启动，要顺手帮你打开验证一下吗？"
                        ),
                        actions: vec![
                            act("launch", "帮我打开验证"),
                            act("finish", "不用了，这就好"),
                        ],
                        facts: v,
                    }
                }
            } else if installed && !complete {
                FlowState {
                    recipe: state.recipe.clone(),
                    stage: Stage::AwaitingUser,
                    say: "主程序在，但有文件缺失。可能还没装完，再等等？".into(),
                    actions: vec![
                        act("check", "再看看"),
                        act("cancel", "算了"),
                    ],
                    facts: v,
                }
            } else {
                FlowState {
                    recipe: state.recipe.clone(),
                    stage: Stage::AwaitingUser,
                    say: "还没找到安装痕迹。\n是不是安装窗口还开着？装完再跟我说。".into(),
                    actions: vec![
                        act("check", "再看看"),
                        act("cancel", "算了"),
                    ],
                    facts: v,
                }
            }
        }

        // 装完后顺手启动并检测进程是否拉起
        (Stage::AwaitingUser, "launch") => {
            let lv = super::detect::launch_and_verify(&state.recipe);
            let running = lv.get("running").and_then(|x| x.as_bool()).unwrap_or(false);
            let msg = lv
                .get("message")
                .and_then(|x| x.as_str())
                .unwrap_or("已尝试打开。");
            FlowState {
                recipe: state.recipe.clone(),
                stage: Stage::Done,
                say: if running {
                    format!("{} 已启动并在运行（{msg}）。", r.name)
                } else {
                    format!("已触发打开 {}：{msg}", r.name)
                },
                actions: vec![],
                facts: lv,
            }
        }

        (Stage::AwaitingUser, "finish") => FlowState {
            recipe: state.recipe.clone(),
            stage: Stage::Done,
            say: format!("好，{} 已经装妥，需要用的时候双击图标就行。", r.name),
            actions: vec![],
            facts: state.facts.clone(),
        },

        // 用户放弃
        (_, "cancel") => FlowState {
            recipe: state.recipe.clone(),
            stage: Stage::Failed,
            say: "好，那先不装了。\n安装包还在下载文件夹里，想装的时候双击它就行。".into(),
            actions: vec![],
            facts: state.facts.clone(),
        },

        // 状态和动作对不上
        (stage, act) => FlowState {
            recipe: state.recipe.clone(),
            stage,
            say: format!("当前状态下不能做这个操作（{act}）。"),
            actions: vec![],
            facts: state.facts.clone(),
        },
    }
}

impl FlowState {
    /// 记录「还等着用户在别的窗口里操作」
    fn with_pending(mut self, pending: bool) -> Self {
        if let Some(o) = self.facts.as_object_mut() {
            o.insert("awaitingExternal".into(), json!(pending));
        }
        self
    }
}

fn fail(recipe: &str, why: &str) -> FlowState {
    FlowState {
        recipe: recipe.to_string(),
        stage: Stage::Failed,
        say: format!("没能继续：{why}"),
        actions: vec![],
        facts: json!({ "error": why }),
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 未知配方立刻失败且说人话() {
        let s = start("nonexistent");
        assert_eq!(s.stage, Stage::Failed);
        assert!(s.say.contains("没有"));
    }

    #[test]
    fn 已装的跳过下载直接告知位置() {
        // 本机装了 Steam，应该是这个路径
        let s = start("steam");
        if s.stage == Stage::AlreadyInstalled {
            assert!(s.say.contains("已经装好了"));
            assert!(s.say.contains("\\") || s.say.contains("/"), "应该带上路径");
            assert!(s.actions.is_empty(), "已装好不该再给操作按钮");
        }
    }

    #[test]
    fn 需要用户确认的阶段一定给出选项() {
        for stage in [Stage::NeedDownload, Stage::NeedConsent, Stage::AwaitingUser] {
            assert!(stage.needs_user_action(), "{stage:?} 应该需要用户操作");
        }
    }

    #[test]
    fn 终态不需要用户操作() {
        for stage in [Stage::AlreadyInstalled, Stage::Done, Stage::Failed] {
            assert!(stage.is_terminal(), "{stage:?} 应该是终态");
        }
    }

    #[test]
    fn 状态动作错配不会崩() {
        let s = start("steam");
        // 拿一个这个阶段不可能有的动作去推进
        let r = advance(&s, "definitely_not_an_action");
        // 要么正常拒绝，要么真的推进了，但不能 panic
        assert!(!r.say.is_empty());
    }

    #[test]
    fn 每一步的说明都不为空() {
        let s = start("steam");
        assert!(!s.say.is_empty());
        for a in &s.actions {
            assert!(!a.label.is_empty(), "动作按钮文字为空");
        }
    }
}
