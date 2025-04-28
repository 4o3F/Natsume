<script setup lang="ts">
import {useMainStore} from "./store.ts";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import {
  useVueTable,
  getCoreRowModel,
  getExpandedRowModel,
  getFilteredRowModel,
  getSortedRowModel, FlexRender, getPaginationRowModel,
} from '@tanstack/vue-table'
import type {
  ColumnDef,
  ColumnFiltersState,
  ExpandedState,
  SortingState,
  VisibilityState,
} from '@tanstack/vue-table'
import {Button} from '@/components/ui/button'
import {getStatus} from "./service.ts";
import {h, ref} from "vue";
import type {AxiosResponse} from "axios";
import {ErrorResponseSchema, type StatusResponse, StatusResponseSchema, type Info} from "./schema.ts";
import Sonner from "@/components/ui/sonner/Sonner.vue";
import {toast} from "vue-sonner";
import {Input} from "@/components/ui/input";
import {valueUpdater} from "@/components/ui/table/utils.ts";
import {RadioGroup, RadioGroupItem} from "@/components/ui/radio-group";
import {Label} from "reka-ui";
import {ArrowUpDown} from "lucide-vue-next";
import DataTablePagination from "@/components/custom/DataTablePagination.vue";

const mainStore = useMainStore()
const newToken = ref<string>('')
const status = ref<null | StatusResponse>(null)


async function setToken() {
  let status = await updateStatus(newToken.value)
  if (status) {
    mainStore.panel_token = newToken.value
    toast.success("Valid token")
  }
}

async function updateStatus(token: string): Promise<boolean> {
  const response: AxiosResponse = await getStatus(token)
  if (response.status !== 200) {
    console.log(response)
    const parsedResponse = ErrorResponseSchema.safeParse(response.data)
    if (!parsedResponse.success) {
      toast.error("Failed to parse error message")
      return false
    } else {
      toast.error(`${parsedResponse.data.error} ${parsedResponse.data.msg}`)
    }
    return false
  }
  // Token valid, saving data

  const parsedResponse = StatusResponseSchema.safeParse(response.data)
  if (!parsedResponse.success) {
    toast.error("Failed to parse status message")
    return false
  }

  status.value = parsedResponse.data
  return true
}

if (mainStore.panel_token !== null) {
  updateStatus(mainStore.panel_token)
  setInterval(() => updateStatus(mainStore.panel_token), 30000)
}

// Data Table
function timestampToTimeString(timestamp: number): string {

  const date = new Date(timestamp * 1000);

  const month = String(date.getMonth() + 1).padStart(2, '0');

  const day = String(date.getDate()).padStart(2, '0');

  const hours = String(date.getHours()).padStart(2, '0');

  const minutes = String(date.getMinutes()).padStart(2, '0');

  const seconds = String(date.getSeconds()).padStart(2, '0');

  return `${month}-${day} ${hours}:${minutes}:${seconds}`;

}

const columns: ColumnDef<Info>[] = [
  {
    accessorKey: 'mac',
    header: 'MAC',
    cell: ({row}) => h('div', row.getValue('mac') === null ? 'N/A' : row.getValue('mac')),
  },
  {
    accessorKey: 'ip',
    header: 'IP',
    cell: ({row}) => h('div', row.getValue('ip') === null ? 'N/A' : row.getValue('ip')),
  },
  {
    accessorKey: 'id',
    header: ({column}) => {
      return h(Button, {
            variant: 'ghost',
            onClick: () => column.toggleSorting(column.getIsSorted() === 'asc'),
          }, () => ['ID', h(ArrowUpDown, {class: 'ml-2 h-4 w-4'})]
      )
    },
    cell: ({row}) => h('div', row.getValue('id') === null ? 'N/A' : row.getValue('id')),
  },
  {
    accessorKey: 'last_seen',
    header: 'Last seen',
    cell: ({row}) => h('div', row.getValue('last_seen') === null ? 'N/A' : timestampToTimeString(row.getValue('last_seen'))),
  },
  {
    accessorKey: 'username',
    header: 'Username',
    cell: ({row}) => h('div', row.getValue('username') === null ? 'N/A' : row.getValue('username')),
  },
  {
    accessorKey: 'password',
    header: 'Password',
    cell: ({row}) => h('div', row.getValue('password') === null ? 'N/A' : row.getValue('password')),
  },
  {
    accessorKey: 'synced',
    header: 'Synced',
    cell: ({row}) => h('div', row.getValue('synced') === null ? 'N/A' : row.getValue('synced')),
  }
]

const sorting = ref<SortingState>([])
const columnFilters = ref<ColumnFiltersState>([])
const columnVisibility = ref<VisibilityState>({})
const rowSelection = ref({})
const expanded = ref<ExpandedState>({})

const table = useVueTable({
  get data() {
    return status.value ? status.value.infos : []
  },
  columns,
  getCoreRowModel: getCoreRowModel(),
  getPaginationRowModel: getPaginationRowModel(),
  getSortedRowModel: getSortedRowModel(),
  getFilteredRowModel: getFilteredRowModel(),
  getExpandedRowModel: getExpandedRowModel(),
  onSortingChange: updaterOrValue => valueUpdater(updaterOrValue, sorting),
  onColumnFiltersChange: updaterOrValue => valueUpdater(updaterOrValue, columnFilters),
  onColumnVisibilityChange: updaterOrValue => valueUpdater(updaterOrValue, columnVisibility),
  onRowSelectionChange: updaterOrValue => valueUpdater(updaterOrValue, rowSelection),
  onExpandedChange: updaterOrValue => valueUpdater(updaterOrValue, expanded),
  state: {
    get sorting() {
      return sorting.value
    },
    get columnFilters() {
      return columnFilters.value
    },
    get columnVisibility() {
      return columnVisibility.value
    },
    get rowSelection() {
      return rowSelection.value
    },
    get expanded() {
      return expanded.value
    },
  },
})

function filterSync(mode: string) {
  switch (mode) {
    case "all":
      table.getColumn("synced")?.setFilterValue(null)
      break
    case "sync":
      table.getColumn("synced")?.setFilterValue(true)
      break
    case "unsync":
      table.getColumn("synced")?.setFilterValue(false)
      break
  }
}

function downloadString(content: string, filename: string, mimeType = "text/plain") {
  const blob = new Blob([content], {type: mimeType});
  const url = URL.createObjectURL(blob);

  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();

  URL.revokeObjectURL(url);
}


function exportIPs(mode: string) {
  switch (mode) {
    case "all":
      let ips = status.value.infos.filter(info => info.ip).map(info => info.ip)
      downloadString(ips.join('\n'), 'ips_all.txt')
      break
    case "unsync":
      let unsync_ips = status.value.infos.filter(info => info.ip && !info.synced).map(info => info.ip)
      downloadString(unsync_ips.join('\n'), 'ips_unsync.txt')
      break
  }
}

function isOffline(last_seen: null | string): boolean {
  if (last_seen === null) {
    return false
  }
  return (Date.now() - (+last_seen) * 1000) > 5 * 60 * 1000
}
</script>

<template>
  <Sonner/>
  <div v-if="mainStore.panel_token !== null" class="flex flex-grow flex-col">
    <div v-if="status !== null" class="flex flex-col gap-3">
      <div class="flex flex-row gap-5 p-3 m-5 justify-between">
        <Card class="flex flex-grow gap-2 bg-lime-300">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">Bind</CardTitle>
            <CardDescription></CardDescription>
          </CardHeader>
          <CardContent>
            <p class="text-2xl font-bold">{{ status.bind_count }}</p>
          </CardContent>
        </Card>
        <Card class="flex flex-grow gap-2 bg-cyan-300">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">Sync</CardTitle>
            <CardDescription></CardDescription>
          </CardHeader>
          <CardContent>
            <p class="text-2xl font-bold">{{ status.sync_count }}</p>
          </CardContent>
        </Card>
        <Card class="flex flex-grow gap-2 bg-indigo-300">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">Info</CardTitle>
            <CardDescription></CardDescription>
          </CardHeader>
          <CardContent>
            <p class="text-2xl font-bold">{{ status.info_count }}</p>
          </CardContent>
        </Card>
        <Card class="flex flex-grow gap-2 bg-amber-300">
          <CardHeader class="pb-2">
            <CardTitle class="text-sm">Not Synced</CardTitle>
            <CardDescription></CardDescription>
          </CardHeader>
          <CardContent>
            <p class="text-2xl font-bold">{{ status.notsync_count }}</p>
          </CardContent>
        </Card>
      </div>
      <div class="p-3 m-5 flex flex-col gap-5">
        <div class="flex flex-col gap-2">
          <p class="text-xl font-bold">Tools</p>
          <div class="flex flex-row gap-3 items-center">
            <p class="font-bold">Sync Status</p>
            <RadioGroup default-value="all" :orientation="'horizontal'" class="flex flex-row"
                        @update:modelValue="filterSync"
            >
              <div class="space-x-2">
                <RadioGroupItem value="all" id="all"/>
                <Label for="all">All</Label>
              </div>
              <div class="space-x-2">
                <RadioGroupItem value="sync" id="sync"/>
                <Label for="sync">Synced Only</Label>
              </div>
              <div class="space-x-2">
                <RadioGroupItem value="unsync" id="unsync"/>
                <Label for="unsync">UnSynced Only</Label>
              </div>
            </RadioGroup>
          </div>
          <div class="flex flex-row gap-3">
            <Button @click="exportIPs('all')">
              Export ALL IPs
            </Button>
            <Button @click="exportIPs('unsync')">
              Export UnSynced IPs
            </Button>
          </div>
        </div>
        <Table>
          <TableHeader>
            <TableRow v-for="headerGroup in table.getHeaderGroups()" :key="headerGroup.id">
              <TableHead v-for="header in headerGroup.headers" :key="header.id">
                <FlexRender v-if="!header.isPlaceholder" :render="header.column.columnDef.header"
                            :props="header.getContext()"/>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <template v-if="table.getRowModel().rows?.length">
              <template v-for="row in table.getRowModel().rows" :key="row.id">
                <TableRow :data-state="row.getIsSelected() && 'selected'" :class="{
                  'bg-amber-500 hover:bg-amber-300': !row.getValue('synced'),
                  'bg-red-500 hover:bg-red-400': isOffline(row.original.last_seen),
                }">
                  <TableCell v-for="cell in row.getVisibleCells()" :key="cell.id">
                    <FlexRender :render="cell.column.columnDef.cell" :props="cell.getContext()"/>
                  </TableCell>
                </TableRow>
                <TableRow v-if="row.getIsExpanded()">
                  <TableCell :colspan="row.getAllCells().length">
                    {{ JSON.stringify(row.original) }}
                  </TableCell>
                </TableRow>
              </template>
            </template>

            <TableRow v-else>
              <TableCell
                  :colspan="columns.length"
                  class="h-24 text-center"
              >
                No results.
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
        <DataTablePagination :table="table" class="w-full"/>
      </div>
    </div>

  </div>
  <div v-else class="flex flex-grow items-center justify-center">
    <Card class="flex w-1/2">
      <CardHeader>
        <CardTitle>Panel Auth</CardTitle>
        <CardDescription>Enter panel auth token</CardDescription>
      </CardHeader>
      <CardContent>
        <Input type="password" placeholder="Token" v-model="newToken"/>
      </CardContent>
      <CardFooter>
        <Button variant="outline" @click="setToken" class="flex-grow">
          Enter
        </Button>
      </CardFooter>
    </Card>
  </div>
</template>

<style scoped>
</style>
