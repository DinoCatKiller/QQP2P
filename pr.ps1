# pr.ps1 - 一键完成「同步 → 开分支 → 提交 → 推送 → 建PR → 合并 → 回main → 删分支」
# 用法:  ./pr.ps1 -m "fix: 修复死锁"
# 说明:
#   - 在 main 上写了代码(未提交)直接跑, 改动会自动带到新分支
#   - 已在功能分支上, 则直接在当前分支提交并开 PR
#   - 合并冲突直接在 GitHub 网页上解决 (Resolve conflicts)
#   - 需要已配置好 gh 登录 (gh auth login)
# 安全保证:
#   - main 上有未提交改动时, 跳过 pull 避免冲突
#   - 任何 git 命令失败立即停止, 本地代码不会丢

param(
    [string]$m,
    [string]$msgFile
)

$ErrorActionPreference = 'Stop'

# 消息来源: 优先用 -msgFile 从 UTF-8 BOM 文件读取(避免中文经命令行传递乱码), 读完即删
if ($msgFile) {
    if (-not (Test-Path $msgFile)) {
        Write-Host "找不到消息文件: $msgFile" -ForegroundColor Red
        exit 1
    }
    $m = Get-Content -Path $msgFile -Raw -Encoding UTF8
    $m = $m.Trim()
    Remove-Item $msgFile -Force -ErrorAction SilentlyContinue
}

if ([string]::IsNullOrWhiteSpace($m)) {
    Write-Host '用法: ./pr.ps1 -m "fix: xxx"   或   ./pr.ps1 -msgFile <消息文件>' -ForegroundColor Yellow
    exit 1
}

function CheckLast {
    if ($LASTEXITCODE -ne 0) {
        Write-Host "上一步失败, 已中止。你的代码改动都还在本地, 不会丢。" -ForegroundColor Red
        exit 1
    }
}

# 统一的 pull: 失败时给冲突解决指引
function GitPull {
    Write-Host "同步远程..." -ForegroundColor Cyan
    git pull
    if ($LASTEXITCODE -ne 0) {
        Write-Host @"
同步时出现冲突或失败! 处理方式:
  - 若提示 CONFLICT (合并冲突): 打开文件找 <<<<<<< ======= >>>>>>> 标记,
    改成最终内容后: git add <文件>; git commit -m "merge: 解决冲突"
  - 若想放弃本次同步: git merge --abort
解决完重新跑脚本即可。你的本地代码不会丢。
"@ -ForegroundColor Red
        exit 1
    }
}

$branch = git rev-parse --abbrev-ref HEAD

if ($branch -eq 'main') {
    # main 上有未提交改动吗?
    $dirty = git status --porcelain
    if ($dirty) {
        Write-Host "检测到 main 上有未提交改动, 直接带到新分支, 跳过 pull 同步。" -ForegroundColor Yellow
    } else {
        GitPull
    }
    # 分支名只取提交信息第一行(标题), 超长截断到 50 字符, 避免超过文件系统限制
    $firstLine = ($m -split "`r?`n")[0]
    $name = ($firstLine -replace '^[\w\u4e00-\u9fa5]+:\s*', '' -replace '[^\w\u4e00-\u9fa5]+', '-').Trim('-')
    if ($name.Length -gt 50) { $name = $name.Substring(0, 50).Trim('-') }
    $branch = "feat/$name"
    git checkout -b $branch
    CheckLast
}

# 提交并推送到远程
git add -A
git commit -m $m
CheckLast
git push -u origin $branch
CheckLast

# 建 PR (分支已有 PR 时会失败, 属正常)
gh pr create --fill
if ($LASTEXITCODE -ne 0) {
    Write-Host "PR 已存在或创建失败, 继续尝试合并现有 PR。" -ForegroundColor Yellow
}

# 合并 PR (有冲突时 gh 会失败, 去 GitHub 网页解决)
# 注意: gh 新版本已移除 --yes, 指定 --merge 后不会再询问, 直接合并
gh pr merge --merge
if ($LASTEXITCODE -ne 0) {
    Write-Host @"
合并失败, 通常是 PR 有冲突。处理方式:
  1. 打开 PR 页面, 点 "Resolve conflicts" 在 GitHub 网页上逐文件解决
  2. 解决完点 "Mark as resolved" 和 "Commit merge"
  3. 然后手动收尾:
       gh pr merge --merge
       git checkout main
       git pull
       git branch -d <分支名>
你的代码都在本地和远程分支上, 不会丢。
"@ -ForegroundColor Red
    exit 1
}

# 回 main, 同步, 删本地分支
git checkout main
GitPull
git branch -d $branch

Write-Host "完成! 分支 $branch 已合并并清理。" -ForegroundColor Green
