import { createApp } from 'vue';
import App from './App.vue';
import './style.css';
import { createDesktopState } from './state/desktopState';

createApp(App).use(createDesktopState()).mount('#app');
