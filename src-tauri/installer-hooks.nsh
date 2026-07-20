; MaxAlert Windows 전환 훅 — Electron(NSIS oneClick) 설치본을 Tauri NSIS 가 대체할 때의 정리 담당.
; electron-updater 는 새 설치기를 `--updated /S --force-run` 으로 실행한다.
; Tauri NSIS 는 /S 만 인식하므로 --force-run 재실행은 POSTINSTALL 훅이 대신한다.
;
; 순서 설계: PREINSTALL 은 실행 중 앱 종료만, Electron 제거는 새 파일 설치가 끝난
; POSTINSTALL 에서 한다 — 설치가 중간에 실패해도 구버전(설치본·자동시작)이 살아남아
; 구 채널로 재시도/복구가 가능하다.

!macro NSIS_HOOK_PREINSTALL
  ; 실행 중인 앱 종료. 이미지명 비교는 대소문자 무시 — Electron(MaxAlert.exe)/Tauri(maxalert.exe) 모두 매칭.
  ; /T 금지: 자기갱신 경로에서 이 설치기가 앱의 자식 프로세스라 /T 는 설치기 자신을 죽인다.
  nsExec::Exec 'taskkill /F /IM "MaxAlert.exe"'
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; ---- 새 파일 설치 성공 후에만 Electron(electron-builder) 설치본 제거 ----
  ; GUID 는 appId(com.makefriends.maxalert)에서 결정적 파생 — 전 머신 동일 (AIXLIFE 실측).
  ; oneClick 설치기는 경로 선택이 없으므로 설치 위치는 전 플릿 고정이다.
  StrCpy $R9 "Software\Microsoft\Windows\CurrentVersion\Uninstall\be734e16-8200-59d6-9d81-4008d7cfb447"
  StrCpy $R8 "$LOCALAPPDATA\Programs\maxalert"
  ${If} ${FileExists} "$R8\Uninstall MaxAlert.exe"
    ; _?= 로 in-place 동기 실행 (자기복사 없이 종료까지 대기, 종료 코드 확인 가능).
    ; deleteAppDataOnUninstall=false 라 %APPDATA%\maxalert\maxalert-data.json (포인트·스트릭)은
    ; 남는다 — Tauri 첫 실행의 Store::load 가 마이그레이션한다.
    ExecWait '"$R8\Uninstall MaxAlert.exe" /currentuser /S _?=$R8' $R7
    ${If} $R7 = 0
      Delete "$R8\Uninstall MaxAlert.exe"
      RMDir /r "$R8"
      DeleteRegKey HKCU "$R9"
    ${EndIf}
    ; Electron 언인스톨러가 같은 이름의 바로가기를 지우므로 다시 만든다
    CreateShortcut "$DESKTOP\MaxAlert.lnk" "$INSTDIR\maxalert.exe"
    CreateShortcut "$SMPROGRAMS\MaxAlert.lnk" "$INSTDIR\maxalert.exe"
  ${EndIf}

  ; Electron setLoginItemSettings 가 남긴 자동시작 잔재 정리.
  ; Tauri 쪽 자동시작은 앱이 기동할 때마다 설정값 기준으로 재적용하므로 지워도 안전하다.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MaxAlert"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "maxalert"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "electron.app.MaxAlert"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "electron.app.maxalert"

  ; 사일런트 설치(자동업데이트 경로)면 앱을 곧바로 재실행 — 항상 켜두는 위젯이라 공백을 없앤다.
  ; RunAsUser: elevate 폴백으로 설치기가 승격돼 있어도 비승격 사용자 토큰으로 실행한다.
  ${If} ${Silent}
    nsis_tauri_utils::RunAsUser "$INSTDIR\maxalert.exe" ""
  ${EndIf}
!macroend
