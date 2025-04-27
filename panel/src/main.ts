import {createApp} from 'vue'
import './style.css'
import App from './App.vue'
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate'
import {createPinia} from "pinia";

const app = createApp(App)

const pinia = createPinia()
pinia.use(piniaPluginPersistedstate)
app.use(pinia)

app.mount('#app')
