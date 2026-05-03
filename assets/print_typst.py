#!/usr/bin/env python3
import subprocess
import sys
import tempfile
import os
import random
import re
from datetime import datetime

# 允许的文件 MIME 类型映射到 Typst 语法高亮名称
ALLOWED_MIME = {
    "text/x-c": "C",
    "text/x-c++": "Cpp",
    "text/x-java-source": "Java",
    "text/x-python": "Python",
    "text/x-script.python": "Python",
    "text/plain": "Python",  # 有些系统将 Python 标记为 text/plain
}

# 打印机 IP 地址列表
#PRINTER_IPS = ['10.12.13.231','10.12.13.232','10.12.13.233','10.12.13.234']
PRINTER_IPS = ['10.12.13.231']

OUTPUT_DIR = "/opt/domjudge/print_backup"

def escape_typst_string(s: str) -> str:
    if s is None:
        return ""
    return s.replace("\\", "\\\\").replace('"', '\\"')


def typst_text(s: str) -> str:
    return f'#text("{escape_typst_string(s)}")'


def raw_block(content: str, lang: str) -> str:
    longest = max((len(match.group(0)) for match in re.finditer(r"`+", content)), default=0)
    fence = "`" * max(3, longest + 1)
    return f"{fence}{lang}\n{content}\n{fence}"


def main():
    # 检查输入参数数量。仅在参数不足时输出简短错误信息。
    if len(sys.argv) < 8:
        print("Error: Missing required script arguments.")
        sys.exit(1)

    file_path, original, language, username, teamname, teamid, location = sys.argv[1:8]
    teamname_text = typst_text(f"Team Name: {teamname}")
    location_text = typst_text(f"Location: {location}")
    original_text = typst_text(f"Source Code: {original}")

    # --- 步骤 1: 检查 MIME 类型 ---
    try:
        # 检测文件 MIME 类型
        mime_type = subprocess.check_output(["file", "-b", "--mime-type", file_path], text=True).strip()
    except subprocess.CalledProcessError:
        print("Error: Failed to detect file type.")
        sys.exit(1)
    except FileNotFoundError:
        print("Error: Required 'file' command not found.")
        sys.exit(1)

    if mime_type not in ALLOWED_MIME:
        print("Error: Unsupported file type. Printing denied.")
        sys.exit(1)

    lang_detected = ALLOWED_MIME[mime_type]

    # --- 步骤 2: 确保输出目录存在 ---
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    # --- 步骤 3: 生成 Typst 模板并编译 PDF ---
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    pdf_filename = f"team{teamid}_{timestamp}.pdf"
    typst_debug_file = os.path.join(OUTPUT_DIR, f"team{teamid}_{timestamp}.typ")
    pdf_file = os.path.join(OUTPUT_DIR, pdf_filename)
    with tempfile.TemporaryDirectory() as tmpdir:
        typst_file = os.path.join(tmpdir, "print.typ")
        # 读取源代码内容
        try:
            with open(file_path, "r", encoding="utf-8", errors="ignore") as src:
                code = src.read()
        except IOError:
            print("Error: Failed to read source file.")
            sys.exit(1)

        code_block = raw_block(code, lang_detected.lower())

        # 构建 Typst 内容
        typst_content = f"""
#set text(font: "Noto Sans CJK SC", size: 9pt)
#set page(
    header: [
        #text(8pt, [{teamname_text}])
        #h(1fr)
        #text(8pt, [{location_text}])
        #line(length: 100%)
    ],
    margin: (x: 1cm, y: 1.5cm)
)

= {original_text}

{code_block}
"""
        # 写入临时文件供编译
        with open(typst_file, "w", encoding="utf-8") as f:
            f.write(typst_content)
        # 编译 Typst 到 PDF
        try:
            # 确保 'typst-cli' 已安装并配置在 PATH 中
            # 成功时不输出任何信息
            typst_cmd = ["/usr/local/bin/typst", "compile", typst_file, pdf_file]
            subprocess.run(typst_cmd, check=True, capture_output=True, text=True)
        except subprocess.CalledProcessError as e:
            try:
                with open(typst_debug_file, "w", encoding="utf-8") as f:
                    f.write(typst_content)
            except IOError as save_error:
                print(f"Error: Failed to save Typst debug file: {save_error}")

            print("Error: Source code compilation failed.")
            print(f"Saved Typst debug file: {typst_debug_file}")
            print(f"Command: {' '.join(typst_cmd)}")
            print(f"Return code: {e.returncode}")
            if e.stdout:
                print(f"Typst stdout:\n{e.stdout}")
            if e.stderr:
                print(f"Typst stderr:\n{e.stderr}")
            sys.exit(1)
        except FileNotFoundError:
            print("Error: Required 'typst' command not found.")
            sys.exit(1)

    # --- 步骤 4: 随机选择打印机并推送打印任务 ---
    
    if not PRINTER_IPS:
        print("Error: No printer addresses configured.")
        sys.exit(1)

    chosen_ip = random.choice(PRINTER_IPS)
    curl_url = f'http://{chosen_ip}:12306/'
    # 移除：print(f"Selected printer IP: {chosen_ip}")

    # 构建 curl 命令
    curl_cmd = [
        'curl',
        '-X', 'POST',
        '--fail-with-body', 
        '--connect-timeout', '10', 
        '--max-time', '30',        
        '-H', 'Content-Type: application/octet-stream',
        '--data-binary', f'@{pdf_file}', 
        curl_url
    ]

    try:
        # 执行 curl 命令
        print_result = subprocess.run(
            curl_cmd, 
            check=True, 
            capture_output=True, 
            text=True
        )
        # --- 步骤 5: 输出最终结果 ---
        # 成功时，仅输出主要的成功信息
        print(f"✅ Print job successfully dispatched")
        # 移除：打印机响应的详细 stdout/stderr
        
    except subprocess.CalledProcessError as e:
        # 打印任务失败，输出简洁的错误信息和服务器响应（如果存在）
        server_response = e.stdout.strip()
        if server_response:
            print(f"❌ Print job failed to dispatch. Server responded: {server_response}")
        else:
            print(f"❌ Print job failed to dispatch. Check network connection or printer status.")
        sys.exit(1)
        
    except FileNotFoundError:
        print("Error: Required 'curl' command not found.")
        sys.exit(1)

if __name__ == "__main__":
    main()
