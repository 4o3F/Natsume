import {defineStore} from "pinia";
import {ref, type Ref} from "vue";

export const useMainStore = defineStore('main',
    () => {
        let panel_token: Ref<null | string> = ref(null)
        return {panel_token}
    },
    {
        persist: true
    }
)