<script setup lang="ts">
import {type Table} from '@tanstack/vue-table'
import {Button} from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import type {Info} from "@/schema.ts";
import {ChevronsLeft, ChevronsRight, ChevronLeft, ChevronRight} from 'lucide-vue-next'

interface DataTablePaginationProps {
  table: Table<Info>
}

defineProps<DataTablePaginationProps>()
</script>

<template>
  <div class="flex items-center justify-between px-2">
    <div class="flex items-center space-x-6 lg:space-x-8 w-full">
      <div class="flex items-center space-x-2">
        <p class="text-sm font-medium">
          Rows per page
        </p>
        <Select
            :model-value="`${table.getState().pagination.pageSize}`"
            @update:model-value="(size) => table.setPageSize(Number(size))"
        >
          <SelectTrigger class="h-8 w-[70px]">
            <SelectValue :placeholder="`${table.getState().pagination.pageSize}`"/>
          </SelectTrigger>
          <SelectContent side="top">
            <SelectItem v-for="pageSize in [10, 20, 30, 40, 50]" :key="pageSize" :value="`${pageSize}`">
              {{ pageSize }}
            </SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div class="flex flex-grow"/>
      <div class="flex w-[100px] items-center justify-center text-sm font-medium">
        Page {{ table.getState().pagination.pageIndex + 1 }} of
        {{ table.getPageCount() }}
      </div>
      <div class="flex items-center space-x-2">
        <Button
            variant="outline"
            class="hidden w-8 h-8 p-0 lg:flex"
            :disabled="!table.getCanPreviousPage()"
            @click="table.setPageIndex(0)"
        >
          <span class="sr-only">Go to first page</span>
          <ChevronsLeft class="w-4 h-4"/>
        </Button>
        <Button
            variant="outline"
            class="w-8 h-8 p-0"
            :disabled="!table.getCanPreviousPage()"
            @click="table.previousPage()"
        >
          <span class="sr-only">Go to previous page</span>
          <ChevronLeft class="w-4 h-4"/>
        </Button>
        <Button
            variant="outline"
            class="w-8 h-8 p-0"
            :disabled="!table.getCanNextPage()"
            @click="table.nextPage()"
        >
          <span class="sr-only">Go to next page</span>
          <ChevronRight class="w-4 h-4"/>
        </Button>
        <Button
            variant="outline"
            class="hidden w-8 h-8 p-0 lg:flex"
            :disabled="!table.getCanNextPage()"
            @click="table.setPageIndex(table.getPageCount() - 1)"
        >
          <span class="sr-only">Go to last page</span>
          <ChevronsRight class="w-4 h-4"/>
        </Button>
      </div>
    </div>
  </div>
</template>