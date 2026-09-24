import { createApp } from "vue";
import App from "./App.vue";
import { installMenuDismissal } from "./features/dismiss-menus";
import { installContextMenuPolicy } from "./features/native-context-menu";

installMenuDismissal();
installContextMenuPolicy();
createApp(App).mount("#root");
