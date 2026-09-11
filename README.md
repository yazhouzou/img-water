# 图片水印清理助手 / Watermark Cleaner

批量去除 AI 生成图片上的水印。**纯本地处理，图片永不上传**。

- 官网（GitHub Pages）：https://yazhouzou.github.io/img-water/ （启用方法见下）
- 下载（免登录）：https://github.com/yazhouzou/img-water/releases/latest

## 功能

- **自动检测**：全图扫描白色/浅色文字水印（含各类 AI 工具常见水印），右下角另有半透明水印自适应兜底
- **手动框选**：深色、彩色、logo 等检测不到的水印，在预览图上框住即可擦除
- **修复式擦除**：LaMa 修复模型按背景纹理智能补全，只处理水印区域，非模糊打码
- **批量处理**：整文件夹一次处理，支持 PNG / JPEG / WebP
- **安全输出**：默认另存到 `watermark-cleaned/`，不覆盖原图；覆盖模式需显式确认并自动备份
- **可取消**：随时取消，已完成图片保持有效
- **前后对比**：拖动分割线滑块查看处理前后差异
- 三端支持：macOS（DMG）/ Windows（NSIS 安装包）/ Android（APK）
- 中英双语 / 深色模式 / 应用内检查更新

## 隐私

所有图片处理均在本机完成，图片与数据不会上传到任何服务器。应用仅联网用于：

1. 首次下载修复模型（LaMa ONNX，约 200MB，仅一次）
2. 检查新版本（GitHub API）

## 使用条款 / 免责声明

请**仅处理你拥有版权或已获授权的图片**。去除他人作品上的水印可能构成侵权，由此产生的法律责任由使用者自行承担。本项目按 "as is" 提供，不附带任何担保。

## 使用

1. 从 Releases 下载安装
2. 首次启动下载修复模型（一次性）
3. 选择文件夹或拖入图片 → 开始处理
4. 结果另存在图片目录下 `watermark-cleaned/`

## CLI（开发者）

```bash
cd desktop/src-tauri && cargo build --release --bin clean-cli
./target/release/clean-cli --root /path/to/images run            # 结果另存 watermark-cleaned/
./target/release/clean-cli --root /path/to/images --overwrite run  # 覆盖原图（自动备份）
./target/release/clean-cli --root /path/to/images --mask-box=-330,-118,-8,-8 run
```

终端快速处理（Python 回退方案）：`./tools/remove_doubao_watermark.py run`

## 公开发布前置（需要账号的接入点）

以下接入点代码已就绪，配置 secrets 后自动生效：

| 事项 | 需要什么 | 配置位置 |
|---|---|---|
| macOS 签名 + 公证 | Apple Developer 账号（$99/年） | Secrets：`APPLE_CERTIFICATE`（.p12 base64）、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`（app-specific）、`APPLE_TEAM_ID` |
| Android release 签名 | 自建 keystore | Secrets：`ANDROID_KEYSTORE_BASE64`（.jks base64）、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、`ANDROID_KEY_PASSWORD`；tag 构建自动出 release APK，未配置保持 debug 签名 |
| Windows 签名 | EV/OV 代码签名证书 | 暂未接入（NSIS 包无签名会有 SmartScreen 提示，可先引导用户"仍要运行"） |
| GitHub Pages 落地页 | 仓库设置 | Settings → Pages → Source 选 `main` 分支 `/docs` 目录 |

## 开发

```bash
cd desktop && pnpm install
pnpm tauri dev            # 桌面调试
pnpm tauri android dev    # Android 真机（需 NDK，见 tools/android-check.sh）
./tools/ui-preview.sh     # UI 浏览器联调（零 SDK，?mobile=1/0 ?nomodel=1）
cd src-tauri && cargo test -- --include-ignored   # 单测 + LaMa E2E（需 .models/lama_fp32.onnx）
```

发布：`./tools/release.sh <x.y.z>`

架构细节与踩坑记录见 [AGENTS.md](AGENTS.md)。
