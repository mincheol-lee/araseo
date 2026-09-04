# Araseo MVP 제품 요구사항 문서

| 항목 | 내용 |
| --- | --- |
| 문서 상태 | MVP 기준선 |
| 제품명 | Araseo (working title) |
| 대상 사용자 | WSL Ubuntu를 주 개발 환경으로 사용하는 개인 개발자 |
| 대상 플랫폼 | Windows 10 1809 이상 또는 Windows 11 + WSL2 Ubuntu |
| 앱 형태 | Windows 네이티브 데스크톱 앱 |
| 주요 진입점 | WSL 터미널에서 `araseo .` |

## 1. 제품 개요

Araseo는 WSL Ubuntu 프로젝트를 편집하고 같은 화면에서 셸과 Codex를 실행하기 위한 개인용 경량 코딩 IDE다. VS Code 수준의 확장성과 언어 기능을 목표로 하지 않고, 파일 탐색·기본 코드 편집·대화형 터미널이라는 반복 사용 빈도가 높은 세 기능을 빠르고 가볍게 제공한다.

Windows 네이티브 창을 사용해 마우스 클릭, 패널 크기 조절, 포커스 이동, 텍스트 선택이 일반 Windows 앱처럼 동작해야 한다. 실제 프로젝트 파일, Git, bash, Codex는 사용자의 WSL Ubuntu 환경에 남겨 두며 Windows 앱은 이를 표시하고 조작하는 호스트 역할만 한다.

## 2. 목표와 성공 기준

### 2.1 목표

- WSL 터미널의 현재 디렉터리를 한 명령으로 연다.
- 파일 트리, 탭 기반 편집기, 실제 대화형 터미널을 한 창에 제공한다.
- 터미널에서 기존 WSL 설정을 그대로 사용해 `codex`를 실행한다.
- 일상적인 작은 코드 변경에 충분한 편집 안정성을 제공한다.
- VS Code보다 명확히 작은 실행 자원과 짧은 시작 시간을 유지한다.

### 2.2 정량 성공 기준

측정은 최적화된 릴리스 빌드, 앱 최초 실행이 아닌 warm WSL 상태, 1,000개 파일 규모의 로컬 WSL 프로젝트를 기준으로 한다.

- 명령 실행부터 사용 가능한 창 표시까지 2초 이내
- 프로젝트를 연 뒤 아무 작업이 없을 때 앱 유휴 메모리 150MB 이하
- 일반 타이핑부터 화면 반영까지 50ms 이내
- 파일 저장 후 WSL에서 읽은 내용과 편집기 내용이 일치
- Codex의 alternate screen, 색상, 커서 이동, 키 입력 및 창 크기 변경이 정상 동작

## 3. MVP 범위

### 3.1 포함

- 단일 창과 단일 workspace
- 디렉터리 트리 탐색 및 기본 파일 작업
- 여러 파일 탭과 기본 텍스트 편집
- 단일 WSL bash 터미널
- 파일별 Git modified/untracked 표시
- WSL 터미널용 `araseo [PATH]` 실행 래퍼
- 명확한 오류 및 미저장 변경 보호

### 3.2 제외

- syntax highlighting, 자동완성, LSP, 정의 이동 및 진단
- 다중 커서, minimap, 코드 접기, 정규식 검색, 프로젝트 전체 검색
- 다중 workspace와 다중 터미널
- 전용 Codex Agent 패널과 Codex 세션 관리
- diff viewer, stage, commit, branch 변경 등 Git 작업 UI
- 확장 프로그램, 원격 계정, 설정 동기화
- Windows 로컬 프로젝트 및 WSL 이외의 원격 환경

## 4. 핵심 사용자 흐름

### 4.1 프로젝트 열기

1. 사용자가 WSL Ubuntu에서 프로젝트 디렉터리로 이동한다.
2. `araseo .`를 실행한다.
3. 래퍼가 `WSL_DISTRO_NAME`과 정규화된 Linux 절대 경로를 Windows 실행 파일에 전달한다.
4. Araseo가 해당 경로의 접근 가능 여부를 확인하고 Windows 창을 연다.
5. 왼쪽에 프로젝트 트리, 중앙에 빈 편집 영역, 아래에 프로젝트 루트에서 시작된 bash가 표시된다.

PATH를 생략하면 현재 디렉터리를 사용한다. 상대 경로는 래퍼가 Linux 절대 경로로 변환한다. 파일 경로가 전달되면 그 파일의 부모 디렉터리를 workspace로 열고 파일 탭을 함께 연다.

### 4.2 파일 편집

1. 사용자가 트리에서 파일을 클릭해 탭으로 연다.
2. 편집 후 탭과 상태 표시줄에서 미저장 상태를 확인한다.
3. `Ctrl+S`로 저장한다.
4. 저장 성공 후 미저장 표시가 사라지고 Git 상태가 갱신된다.

### 4.3 Codex 실행

1. 사용자가 하단 터미널을 클릭한다.
2. 기존 WSL 셸과 같은 방식으로 `codex`를 입력한다.
3. Araseo는 PTY 입출력과 터미널 화면만 중계하며 Codex의 설치, 인증, 설정을 변경하지 않는다.

## 5. 화면 및 상호작용

### 5.1 기본 레이아웃

```text
+------------------+---------------------------------------------+
| File Tree        | Editor Tabs                                 |
|                  +---------------------------------------------+
|                  |                                             |
|                  | Editor                                      |
|                  |                                             |
|                  +---------------------------------------------+
|                  | Terminal                                    |
+------------------+---------------------------------------------+
| Workspace | Git | Active file | Cursor/encoding | Status       |
+---------------------------------------------------------------+
```

- 파일 트리와 작업 영역 사이, 편집기와 터미널 사이에 드래그 가능한 splitter를 둔다.
- 터미널은 접고 펼칠 수 있으며 마지막 높이를 현재 실행 중에 유지한다.
- 클릭한 패널이 키보드 포커스를 가진다. 포커스는 테두리나 헤더 상태로 구분한다.
- 첫 버전은 어두운 단일 테마와 고정폭 시스템 폰트를 사용한다.
- 창 최소 크기는 800×600이며, 작은 크기에서는 파일 트리와 터미널을 접을 수 있어야 한다.

### 5.2 기본 단축키

| 동작 | 단축키 |
| --- | --- |
| 파일 또는 workspace 열기 | `Ctrl+O` |
| 현재 파일 저장 | `Ctrl+S` |
| 다른 이름으로 저장 | `Ctrl+Shift+S` |
| 현재 파일에서 찾기 | `Ctrl+F` |
| 실행 취소 / 다시 실행 | `Ctrl+Z` / `Ctrl+Y` |
| 현재 탭 닫기 | `Ctrl+W` |
| 터미널 표시 전환 | `` Ctrl+` `` |

터미널에 포커스가 있을 때 셸 또는 Codex가 사용하는 키 조합은 터미널로 전달한다. 전역 UI 단축키는 터미널 표시 전환처럼 명시된 최소 항목만 먼저 처리한다.

## 6. 기능 요구사항

### 6.1 Workspace와 파일 트리

- FR-FS-01: 앱은 하나의 WSL 배포판과 하나의 Linux 루트 경로를 workspace로 유지한다.
- FR-FS-02: 폴더는 필요할 때 자식을 읽는 lazy loading 방식으로 확장한다.
- FR-FS-03: 파일 한 번 클릭은 탭을 열고, 이미 열린 파일이면 해당 탭으로 전환한다.
- FR-FS-04: 새 파일, 새 폴더, 이름 변경, 삭제를 지원한다. 삭제 전에는 대상의 Linux 경로를 표시하고 확인을 받는다.
- FR-FS-05: `.git` 디렉터리는 기본적으로 숨긴다. 그 밖의 dotfile은 표시한다.
- FR-FS-06: 수동 새로고침은 확장 상태와 열린 탭을 가능한 한 유지한다.
- FR-FS-07: 심볼릭 링크는 표시하지만 workspace 밖을 가리키는 디렉터리 링크는 따라가지 않는다.
- FR-FS-08: 권한 오류와 사라진 파일은 앱 종료 없이 해당 항목 수준에서 표시한다.

### 6.2 편집기와 문서 수명 주기

- FR-ED-01: 파일은 탭 단위로 열며 같은 정규화 경로를 중복해서 열지 않는다.
- FR-ED-02: 단일 커서, 마우스 및 Shift 선택, 복사, 잘라내기, 붙여넣기를 지원한다.
- FR-ED-03: undo/redo 기록은 문서별로 유지하고 저장 후에도 탭을 닫기 전까지 유지한다.
- FR-ED-04: 줄 번호를 표시하고 word wrap은 기본적으로 끈다.
- FR-ED-05: 찾기는 현재 파일의 일반 문자열을 대상으로 하며 다음/이전 결과 이동과 대소문자 구분 옵션을 제공한다.
- FR-ED-06: 탭 문자는 실제 `\t`로 입력하며 기본 표시 너비는 4칸이다.
- FR-ED-07: UTF-8 텍스트만 편집한다. UTF-8로 해석할 수 없거나 NUL 바이트를 포함하면 편집 대신 지원하지 않는 파일 안내를 표시한다.
- FR-ED-08: 2MiB를 초과하는 파일은 읽고 쓰지 않고 파일 크기 제한 안내를 표시한다.
- FR-ED-09: 원본의 LF 또는 CRLF 줄바꿈을 보존한다. 새 파일은 LF를 사용한다.
- FR-ED-10: 저장은 같은 디렉터리의 임시 파일 작성과 교체 방식으로 수행해 부분 저장을 방지한다. 실패 시 원본을 유지한다.
- FR-ED-11: 열린 시점의 파일 크기와 수정 시간을 보관한다. 저장 전에 외부 변경이 발견되면 `다시 불러오기`, `덮어쓰기`, `취소` 중 하나를 선택하게 한다.
- FR-ED-12: 미저장 탭 닫기와 앱 종료 시 `저장`, `저장하지 않음`, `취소`를 제공한다.
- FR-ED-13: 파일이 외부에서 삭제되면 탭 내용을 즉시 버리지 않고 삭제 상태와 다른 이름으로 저장할 기회를 제공한다.

### 6.3 내장 터미널

- FR-TR-01: workspace를 열 때 단일 ConPTY 세션을 만들고 다음 의미의 명령으로 로그인 셸을 시작한다.

  ```text
  wsl.exe -d <distro> --cd <linux-workspace-root> bash -l
  ```

- FR-TR-02: 터미널은 256색과 true color, bold/italic/underline, Unicode, 커서 표시, alternate screen을 처리한다.
- FR-TR-03: 터미널 픽셀 크기에서 행과 열을 계산하고 splitter 또는 창 크기 변경 시 ConPTY와 화면 모델을 함께 resize한다.
- FR-TR-04: 키 입력, 제어키, 붙여넣기를 UTF-8 및 적절한 VT 입력 시퀀스로 전달한다.
- FR-TR-05: 최대 10,000줄의 스크롤백을 유지하고 초과분은 오래된 줄부터 제거한다.
- FR-TR-06: 드래그는 기본적으로 로컬 텍스트 선택에 사용한다. 실행 중인 앱이 mouse reporting을 요청하면 일반 클릭과 휠을 전달하고 `Shift+드래그`는 로컬 선택으로 유지한다.
- FR-TR-07: 셸이 종료되면 종료 코드를 표시하고 `다시 시작` 동작을 제공한다.
- FR-TR-08: 앱 종료 시 PTY 입력을 닫고 자식 프로세스 종료를 기다린 뒤, 제한 시간 이후 남은 호스트 프로세스를 정리한다.

### 6.4 Git 상태

- FR-GIT-01: Windows Git이 아닌 workspace 배포판의 `git`을 실행한다.
- FR-GIT-02: `git status --porcelain=v2 -z` 결과를 파싱해 modified와 untracked 상태를 파일 트리에 표시한다.
- FR-GIT-03: 프로젝트 열기, 파일 저장, 터미널에서 UI로 포커스 복귀, 수동 새로고침 때 상태를 갱신한다.
- FR-GIT-04: Git 저장소가 아니거나 Git이 설치되지 않아도 편집 및 터미널 기능은 정상 동작한다.
- FR-GIT-05: stage, commit, checkout 등 저장소를 변경하는 Git 명령은 UI에서 실행하지 않는다.

## 7. 기술 설계

### 7.1 기술 스택

| 영역 | 선택 | 이유 |
| --- | --- | --- |
| 언어 | Rust stable | 단일 네이티브 실행 파일, 명확한 자원 소유권, 낮은 런타임 오버헤드 |
| GUI | Slint | Rust와 직접 결합되고 Electron/WebView 런타임이 필요 없음 |
| 창/렌더링 | Slint `winit` + FemtoVG | Windows 네이티브 창과 GPU 렌더링, Skia보다 작은 의존 범위 |
| 편집 컨트롤 | Slint `TextEdit` 기반 래퍼 | MVP 편집 범위를 충족하면서 코드 에디터 전체를 새로 구현하지 않음 |
| PTY | `portable-pty` | Windows ConPTY 생성과 프로세스 입출력 수명 주기 추상화 |
| 터미널 모델 | `vt100` | ANSI/VT 바이트를 화면 cell grid, 색상, 커서, 모드로 변환 |
| 비동기 작업 | Rust thread/channel | UI event loop에서 파일·PTY·Git I/O를 분리하고 런타임 크기를 제한 |

Slint의 `TextEdit`는 다중행 입력, 선택, 클립보드와 키 이벤트를 제공하지만 코드 편집기 기능은 제공하지 않는다. MVP에서는 이를 감싼 문서 모델로 찾기, 저장 상태, 제한된 undo/redo를 구현한다. 향후 syntax highlighting이나 LSP가 필요해질 때 편집기 계층을 별도로 재평가한다.

참고 자료:

- [Slint TextEdit](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/textedit/)
- [Slint backends and renderers](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/)
- [Microsoft ConPTY](https://learn.microsoft.com/en-us/windows/console/pseudoconsoles)
- [vt100 crate](https://docs.rs/vt100/latest/vt100/)

### 7.2 프로세스 경계

```text
WSL Ubuntu                                  Windows
┌─────────────────────┐                    ┌──────────────────────────┐
│ araseo shell wrapper  │ --distro/root-->  │ Araseo.exe + Slint UI     │
│                     │                    │                          │
│ project files       │ <----- UNC -----> │ filesystem/document core│
│ git                 │ <--- wsl.exe ---- │ Git status worker       │
│ bash / Codex        │ <--- ConPTY ----> │ terminal worker + vt100 │
└─────────────────────┘                    └──────────────────────────┘
```

Windows 앱은 Linux 절대 경로를 다음 규칙으로 UNC 경로에 매핑한다.

```text
/home/minch/project
→ \\wsl.localhost\Ubuntu\home\minch\project
```

UI 스레드는 Slint 모델 갱신과 입력 처리만 담당한다. 파일 열기·저장, Git 조회, PTY 읽기는 worker에서 수행하고 channel을 통해 결과를 UI 스레드로 전달한다. 오래 걸리는 작업 중에도 편집기와 패널 클릭이 멈추지 않아야 한다.

### 7.3 공개 CLI 계약

사용자용 인터페이스:

```text
araseo [PATH]
araseo --help
araseo --version
```

Windows 실행 파일에 전달되는 내부 인터페이스:

```text
Araseo.exe --distro <WSL_DISTRO_NAME> --root <LINUX_ABSOLUTE_PATH> [--file <LINUX_ABSOLUTE_PATH>]
```

- `araseo` 래퍼는 WSL 전용 POSIX shell script다.
- `PATH` 기본값은 현재 디렉터리다.
- distro 이름이나 경로는 문자열 결합 후 셸에서 재평가하지 않고 각각 하나의 인자로 전달한다.
- 이미 같은 workspace 창이 실행 중인 경우의 재사용은 MVP에서 보장하지 않는다. 각 호출은 새 창을 열 수 있다.

### 7.4 핵심 도메인 타입

```rust
struct Workspace {
    distro: String,
    linux_root: PathBuf,
    unc_root: PathBuf,
}

struct Document {
    linux_path: PathBuf,
    text: String,
    line_ending: LineEnding,
    dirty: bool,
    disk_revision: DiskRevision,
}

struct FileNode {
    name: String,
    linux_path: PathBuf,
    kind: FileKind,
    expanded: bool,
    git_status: GitStatus,
}

enum GitStatus { Clean, Modified, Untracked }

struct TerminalSession {
    rows: u16,
    columns: u16,
    running: bool,
    exit_code: Option<i32>,
}

struct TerminalCell {
    text: String,
    foreground: TerminalColor,
    background: TerminalColor,
    attributes: CellAttributes,
}
```

UI는 이 상태의 읽기 전용 view model을 소비하고 `open_file`, `save_document`, `resize_terminal`, `write_terminal_input` 같은 명시적 command callback을 Rust 코어에 보낸다. UI 파일에서 직접 파일이나 프로세스를 조작하지 않는다.

## 8. 오류 처리와 안전성

| 상황 | 동작 |
| --- | --- |
| WSL 또는 지정 distro 없음 | 창에 원인과 확인할 명령을 표시하고 workspace를 열지 않음 |
| UNC 경로 접근 실패 | Linux 경로와 distro를 표시하고 다시 시도 제공 |
| 파일 읽기/저장 권한 없음 | 해당 탭에 오류를 표시하고 기존 편집 내용을 유지 |
| 외부 파일 변경 | 다시 불러오기, 덮어쓰기, 취소 선택 제공 |
| Git 없음/저장소 아님 | Git 표시만 비활성화하고 나머지는 계속 사용 |
| `codex` 없음 | bash의 일반 command-not-found 출력을 그대로 표시 |
| PTY 또는 bash 종료 | 종료 코드와 다시 시작 버튼 표시 |
| 터미널 출력 폭주 | UI 갱신을 batch 처리하고 스크롤백 상한 적용 |

- 모든 파일 작업은 workspace 내부의 정규화된 경로만 허용한다. 심볼릭 링크를 통한 workspace 탈출을 차단한다.
- shell command 문자열에 경로를 삽입하지 않고 OS 인자 배열로 전달해 공백, 한글, 따옴표가 있는 경로를 안전하게 처리한다.
- 앱은 Codex 인증 토큰이나 셸 환경 변수를 별도로 읽거나 저장하지 않는다.
- 삭제는 복구 불가능할 수 있으므로 항상 명시적으로 확인하며, 디렉터리는 비어 있지 않으면 대상 수를 함께 경고한다.

## 9. 검증 계획

### 9.1 자동 테스트

- Linux 경로와 UNC 경로 상호 변환: 공백, 한글, 점 파일, 긴 경로
- workspace 경계 검사와 `..` 및 심볼릭 링크 탈출 방지
- UTF-8 판별, NUL 탐지, 2MiB 제한, LF/CRLF 보존
- 문서 dirty 상태, undo/redo, 저장 revision 및 외부 변경 판정
- `git status --porcelain=v2 -z`의 modified/untracked 파싱
- 키 이벤트의 VT 입력 시퀀스 변환
- VT 색상, cursor, alternate screen, resize 상태 처리
- 터미널 스크롤백 10,000줄 상한

### 9.2 Windows–WSL 통합 테스트

- `araseo .`, 상대 경로, 절대 경로 및 단일 파일로 실행
- 파일 생성, 이름 변경, 편집, 원자적 저장, 삭제 후 WSL에서 결과 확인
- WSL에서 열린 파일을 외부 변경한 뒤 충돌 선택지 각각 검증
- Git 저장소와 비저장소, Git 미설치 환경에서 동작 확인
- bash에서 `codex` 실행 후 입력, 색상, alternate screen, 붙여넣기, 스크롤, resize 확인
- 셸의 정상 종료, 비정상 종료 및 터미널 재시작 확인
- 앱 종료 시 미저장 문서와 실행 중 PTY 정리 확인

### 9.3 성능 테스트

- 1,000개 파일 프로젝트의 시작 시간과 초기 트리 표시 시간
- 빈 workspace, 10개 열린 탭, Codex 실행 상태의 메모리 사용량
- 빠른 연속 타이핑과 대량 터미널 출력 중 UI 입력 지연
- 큰 디렉터리 lazy loading과 Git 상태 갱신 중 UI 응답성

## 10. 출시 단계

### MVP

이 문서의 파일 트리, 기본 편집기, 단일 터미널, Git 상태 표시와 `araseo .` 실행 흐름을 완성한다.

### 2단계 후보

- tree-sitter 기반 syntax highlighting
- 다중 터미널 탭
- 읽기 전용 Git diff viewer
- 최근 workspace와 기본 UI 설정 저장

### 3단계 후보

- LSP 자동완성, 진단, 정의 이동
- Codex Agent 패널, 세션 상태, 작업 승인 UI
- Git stage/commit UI

Slint 편집기가 실제 사용성, IME, 대용량 문서 또는 렌더링 성능 합격 기준을 충족하지 못하면 Monaco/Tauri 전환을 별도 기술 결정으로 검토한다. MVP 단계에서는 두 UI 스택을 동시에 유지하지 않는다.

## 11. MVP 완료 조건

다음 조건이 모두 충족되면 MVP를 완료한 것으로 본다.

- WSL Ubuntu에서 `araseo .`로 Windows 네이티브 창이 열린다.
- 파일 트리에서 프로젝트를 탐색하고 여러 UTF-8 파일을 안전하게 편집·저장할 수 있다.
- 미저장 종료와 외부 변경으로 인해 사용자 편집 내용이 묵시적으로 유실되지 않는다.
- 같은 창의 터미널에서 bash와 Codex를 대화형으로 사용할 수 있다.
- Git modified/untracked 상태가 파일 트리에 표시된다.
- 오류 상황이 전체 앱 crash 대신 사용자가 이해할 수 있는 상태로 표시된다.
- 정량 성능 목표와 자동·통합 테스트가 통과한다.
