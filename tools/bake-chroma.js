// 구버전 siren.html의 실시간 크로마키 로직을 그대로 오프라인 베이크.
// stdin: rawvideo rgba → 픽셀 변환 → stdout: rawvideo rgba
const W = 834, H = 1112, FRAME = W * H * 4

function key(f) {
  for (let i = 0; i < f.length; i += 4) {
    const r = f[i], g = f[i + 1], b = f[i + 2]
    // 초록 지배 픽셀 → 투명 (어두운 비네트 그린까지 커버)
    if (g > 55 && g > r * 1.2 && g > b * 1.2) {
      f[i + 3] = 0
    } else if (g > 45 && g > r * 1.05 && g > b * 1.05) {
      // 경계 픽셀은 반투명 + 초록 번짐 제거
      f[i + 3] = Math.min(f[i + 3], 110)
      f[i + 1] = Math.max(r, b)
    } else if (g > r && g > b) {
      f[i + 1] = Math.max(r, b)
    }
  }
}

let pending = Buffer.alloc(0)
process.stdin.on('data', chunk => {
  pending = pending.length ? Buffer.concat([pending, chunk]) : chunk
  let backpressure = false
  while (pending.length >= FRAME) {
    const frame = Buffer.from(pending.subarray(0, FRAME))
    pending = pending.subarray(FRAME)
    key(frame)
    if (!process.stdout.write(frame)) backpressure = true
  }
  if (backpressure) {
    process.stdin.pause()
    process.stdout.once('drain', () => process.stdin.resume())
  }
})
process.stdin.on('end', () => process.stdout.end())

// 사용법 (그린스크린 mp4 → 투명 webm):
// ffmpeg -i assets/chick.mp4 -f rawvideo -pix_fmt rgba - \
//   | node tools/bake-chroma.js \
//   | ffmpeg -y -f rawvideo -pix_fmt rgba -s 834x1112 -r 24 -i - \
//     -c:v libvpx-vp9 -pix_fmt yuva420p -crf 22 -b:v 0 -deadline good -cpu-used 2 -an assets/chick.webm
// (해상도가 다른 영상이면 위의 W/H 상수와 -s 값을 맞출 것)
