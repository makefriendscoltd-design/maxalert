const fs = require('fs')
const path = require('path')

const THIRTY_DAYS = 30 * 24 * 60 * 60 * 1000

class Store {
  constructor(file) {
    this.file = file
    this.data = {
      todos: [],
      settings: {
        notionToken: '',
        notionDb: '',
        openAtLogin: false,
        sirenVolume: 0.5,
        postitTheme: 'classic',
        unlockedThemes: ['classic']
      },
      streak: { count: 0, lastDate: null },
      lastRewardDate: null,
      points: { total: 0, ledger: [] },
      stats: { totalDone: 0, sirenSaves: 0 },
      badges: []
    }
    try {
      const loaded = JSON.parse(fs.readFileSync(file, 'utf8'))
      this.data = {
        ...this.data,
        ...loaded,
        settings: { ...this.data.settings, ...(loaded.settings || {}) },
        streak: { ...this.data.streak, ...(loaded.streak || {}) },
        points: { ...this.data.points, ...(loaded.points || {}) },
        stats: { ...this.data.stats, ...(loaded.stats || {}) },
        badges: loaded.badges || []
      }
    } catch { /* first run */ }
    // 30일 지난 항목 정리
    const cutoff = Date.now() - THIRTY_DAYS
    this.data.todos = this.data.todos.filter(t => (t.createdAt || 0) >= cutoff)
  }

  save() {
    fs.mkdirSync(path.dirname(this.file), { recursive: true })
    fs.writeFileSync(this.file, JSON.stringify(this.data, null, 2))
  }

  todosOn(dateStr) {
    return this.data.todos.filter(t => t.date === dateStr)
  }

  find(id) {
    return this.data.todos.find(t => t.id === id)
  }
}

module.exports = Store
