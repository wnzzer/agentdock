import { createApp } from "vue";
import App from "./App.vue";
import { installMenuDismissal } from "./features/dismiss-menus";

installMenuDismissal();
createApp(App).mount("#root");
