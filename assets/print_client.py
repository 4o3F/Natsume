import secrets
import shutil
import subprocess
import time
from pathlib import Path
from wsgiref.simple_server import make_server

BASE_DIR = Path(__file__).resolve().parent
SUMATRA_PATH = BASE_DIR / "SumatraPDF" / "SumatraPDF.exe"
LOG_FILE = BASE_DIR / "Print.log"
SUCCESS_DIR = BASE_DIR / "Success"
ERROR_DIR = BASE_DIR / "Error"
TEMP_DIR = BASE_DIR / "Temp"
CHARSET = "23456789qwertyupasdfghjkzxcvbnm"
PORT = 12306


def random_filename(length=10):
    return "".join(secrets.choice(CHARSET) for _ in range(length)) + ".pdf"


def print_target_file(filename):
    try:
        filename = filename.resolve()
        print(f"PDF path before print: {filename}")
        print(f"PDF exists before print: {filename.exists()}")
        print(f"PDF size before print: {filename.stat().st_size if filename.exists() else 'missing'}")
        print(f"SumatraPDF path: {SUMATRA_PATH}")
        print(f"SumatraPDF exists: {SUMATRA_PATH.exists()}")

        print_cmd = [
            str(SUMATRA_PATH),
            "-print-to-default",
            "-silent",
            str(filename),
        ]
        result = subprocess.run(
            print_cmd,
            cwd=BASE_DIR,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Print failed for {filename.name} with return code {result.returncode}")
            print(f"Print command: {print_cmd!r}")
            if result.stdout:
                print(f"SumatraPDF stdout:\n{result.stdout}")
            if result.stderr:
                print(f"SumatraPDF stderr:\n{result.stderr}")
            return False

        print(f"Submitted print job for {filename.name}")
        return True
    except Exception as exc:
        print(f"Print failed for {filename.name}: {exc}")
        return False


def log_status(filename, success):
    target_dir = SUCCESS_DIR if success else ERROR_DIR
    target_dir.mkdir(exist_ok=True)
    current_date = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime())
    status = "Success" if success else "Failed"

    with LOG_FILE.open("a", encoding="utf-8") as log_file:
        log_file.write(f"{current_date} {status}: {filename.name}\n")

    try:
        shutil.copy2(filename, target_dir / filename.name)
    except OSError as exc:
        with LOG_FILE.open("a", encoding="utf-8") as log_file:
            log_file.write(f"{current_date} Copy failed: {filename.name}: {exc}\n")


def read_content_length(environ):
    try:
        return max(0, int(environ.get("CONTENT_LENGTH") or 0))
    except ValueError:
        return 0


def application(environ, start_response):
    request_length = read_content_length(environ)
    TEMP_DIR.mkdir(exist_ok=True)
    filename = TEMP_DIR / random_filename()

    with filename.open("wb") as output_file:
        output_file.write(environ["wsgi.input"].read(request_length))

    success = print_target_file(filename)
    log_status(filename, success)

    start_response("200 OK", [("Content-Type", "text/plain; charset=utf-8")])
    return [b"Successful."]


if __name__ == "__main__":
    httpd = make_server("0.0.0.0", PORT, application)
    print(f"serving http on port {PORT}...")
    httpd.serve_forever()
