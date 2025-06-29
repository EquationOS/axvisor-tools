#pragma once

#include <linux/kernel.h>
#include <linux/limits.h>
#include <linux/list.h>
#include <linux/memory.h>
#include <linux/types.h>

#define UINT64_MAX (~(uint64_t)0)

#define SHM_REGION_SIZE (0x40000000) // 1 GB shared memory region size
#define MAX_SHM_PAGES                                                          \
	(SHM_REGION_SIZE / PAGE_SIZE) // Maximum number of 4KB pages in the region
#define BITMAP_SIZE                                                            \
	(MAX_SHM_PAGES / 64) // Number of 64-bit integers needed for bitmaps

typedef struct eqshm
{
	void *base;
	uint64_t bitmaps[BITMAP_SIZE];
	struct list_head list;
} eqshm_t;

void *allocate_shm_page(eqshm_t *region);
void *allocate_contiguous_shm_pages(eqshm_t *region, int page_num);
void free_shm_page(eqshm_t *region, void *page);
void free_contiguous_shm_pages(eqshm_t *region, void *page, int page_num);

int is_shm_region_empty(eqshm_t *region);
int is_shm_region_full(eqshm_t *region);

eqshm_t *allocate_new_shm_region(struct list_head *region_list, void *base);
void release_shm_region(eqshm_t *region, struct list_head *region_list);