const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('api', {
  listTodos: () => ipcRenderer.invoke('todos:list'),
  addTodo: (t) => ipcRenderer.invoke('todos:add', t),
  updateTodo: (id, patch) => ipcRenderer.invoke('todos:update', id, patch),
  toggleTodo: (id) => ipcRenderer.invoke('todos:toggle', id),
  deleteTodo: (id) => ipcRenderer.invoke('todos:delete', id),
  postponeTodo: (id, minutes) => ipcRenderer.invoke('todos:postpone', id, minutes),
  getSettings: () => ipcRenderer.invoke('settings:get'),
  setSettings: (s) => ipcRenderer.invoke('settings:set', s),
  syncNotion: () => ipcRenderer.invoke('notion:sync'),
  buyTheme: (id) => ipcRenderer.invoke('shop:buyTheme', id),
  openDashboard: () => ipcRenderer.invoke('dashboard:open'),
  closeReward: () => ipcRenderer.invoke('reward:close'),
  setMouseThrough: (ignore) => ipcRenderer.send('postit:mouse', ignore),
  onTodos: (cb) => ipcRenderer.on('todos', (_e, data) => cb(data)),
  onSirenTodo: (cb) => ipcRenderer.on('siren:todo', (_e, data) => cb(data))
})
