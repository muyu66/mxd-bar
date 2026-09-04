//! 编译期把 `app.rc`（exe 图标 + 版本信息 2.0.0）编进二进制。
//! 用 embed-resource 定位系统 rc.exe 把 .rc 编译成 .res 再链接进 exe。

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    embed_resource::compile("app.rc", embed_resource::NONE);
}
