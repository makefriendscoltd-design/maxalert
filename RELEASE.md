# MaxAlert 릴리스 절차 (v0.2.4부터)

빌드는 CI가 한 벌만 만들고, 게시(=전 플릿 자동업데이트 시작)는 사람이 클릭한다.

## 새 버전 내는 법 (전체 절차)

1. 코드 수정 → `src-tauri/tauri.conf.json` 의 `version` 올리기 → master 머지
2. 태그 푸시:
   ```bash
   git tag v0.2.4 && git push origin v0.2.4
   ```
3. GitHub Actions `release` 워크플로우가 자동으로:
   - 태그와 conf 버전 일치 검사 (다르면 빌드 실패)
   - 윈도우 NSIS exe + latest.yml, 맥 aarch64 dmg, SHA256SUMS 빌드
   - `maxalert-releases` 에 **드래프트** 릴리스 생성 (이 상태에선 아무 영향 없음)
4. 드래프트에서 파일 받아 동작 확인(선택) → 릴리스 페이지에서 **Publish release** 클릭
   - 클릭하는 순간: 전 윈도우 플릿 자동업데이트 + 카탈로그 다운로드가 새 버전으로 전환
   - CLI 로도 가능: `gh release edit v0.2.4 --repo makefriendscoltd-design/maxalert-releases --draft=false`

## 카탈로그(api.aimax.ai.kr) 연동

- 카탈로그 다운로드 URL 은 GitHub Releases 자산을 직접 가리킨다 (Oracle 업로드 불필요)
- 새 버전 게시 후 `oracle/aimax-reports-api/server.js` (AIMAX-AI-Staff-Management repo) 의
  맥스 항목 URL 버전만 바꿔 배포하면 끝

## 드라이런 (릴리스 없이 빌드만 검증)

Actions 탭 → `release` → Run workflow (master). 태그가 아니므로 드래프트 릴리스는 만들지 않는다.

## 주의

- `windows-nsis.yml` (mac/tauri 브랜치용 빌드 검증) 은 그대로 유지 — 릴리스와 무관
- 수동 빌드로 릴리스에 파일 올리지 말 것 — "같은 버전 다른 바이너리" 재발 방지가 이 구조의 목적
- 크로스 repo 발행 토큰: maxalert repo Actions secret `RELEASES_TOKEN` (만료 시 릴리스 job 만 실패 — 재설정 후 re-run)
