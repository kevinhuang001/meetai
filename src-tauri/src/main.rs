// Windows 发布版不弹控制台窗口。
// 注意：`--check` 诊断模式的输出仍然可以重定向到文件（没有控制台也能打印）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 诊断模式：不启动图形界面，直接打印自检结果后退出
    if args.iter().any(|a| a == "--check" || a == "-c" || a == "check") {
        std::process::exit(meeting_hear_lib::diagnostics::run_check(&args));
    }
    meeting_hear_lib::run()
}
