import axios, {type AxiosInstance} from "axios";

const api = createBaseAPI()

function createBaseAPI(): AxiosInstance {
    return axios.create({
        baseURL: "/",
        // baseURL: "https://localhost:2333/",
        headers: {},
        withCredentials: false,
        adapter: 'fetch',
        validateStatus: function () {
            return true;
        }
    })
}

export function getStatus(token: string) {
    return api.get("/status", {
        headers: {
            "token": token
        }
    })
}

export function removeBindByMAC(mac: string, token: string) {
    return api.post("/unbind", {
        "mac": mac
    }, {
        headers: {
            "token": token
        }
    },)
}