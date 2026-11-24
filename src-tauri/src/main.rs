// Windows 子系统入口：不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    local_ai_helper_lib::run()
}
