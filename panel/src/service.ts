import axios, {type AxiosInstance} from "axios";

const api = createBaseAPI()

function createBaseAPI(): AxiosInstance {
    return axios.create({
        baseURL: "/",
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