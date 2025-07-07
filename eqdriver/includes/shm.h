#pragma once

#include <linux/kernel.h>
#include <linux/limits.h>
#include <linux/list.h>
#include <linux/memory.h>
#include <linux/types.h>

#define UINT64_MAX (~(uint64_t)0)

#define SCF_MAGIC_NUMBER (0x45534346) // "ESCF"

#define MAX_SHM_REGION_SIZE                                                    \
	(0x200000) // Maximum size of the shared memory region (2MB)
#define MAX_SHM_PAGES                                                          \
	(MAX_SHM_REGION_SIZE /                                                     \
	 PAGE_SIZE) // Maximum number of 4KB pages in the region
#define BITMAP_SIZE                                                            \
	(MAX_SHM_PAGES / 64) // Number of 64-bit integers needed for bitmaps

typedef struct eqshm
{
	void *base;
	uint64_t size; // Size of the shared memory region
	uint64_t bitmaps[BITMAP_SIZE];
	struct list_head list;
} eqshm_t;

typedef struct eqscf_queue_region
{
	uint64_t pid;
	uint64_t host_pid;
	uint64_t base_gpa;
	uint64_t size;
	struct list_head list;
} eqscf_queue_region_t;

void *allocate_shm_page(eqshm_t *region);
void *allocate_contiguous_shm_pages(
	eqshm_t *region, int page_num, int *allocated_pages);
void free_shm_page(eqshm_t *region, void *page);
void free_contiguous_shm_pages(eqshm_t *region, void *page, int page_num);

eqscf_queue_region_t *get_scf_queue_region_by_host_pid(
	struct list_head *scf_region_list, uint64_t pid);

int is_shm_region_empty(eqshm_t *region);
int is_shm_region_full(eqshm_t *region);

eqshm_t *allocate_new_page_cache_region(
	struct list_head *region_list, void *base, uint64_t size);
void release_page_cache_region(eqshm_t *region, struct list_head *region_list);
void cleanup_page_cache_region(eqshm_t *region, struct list_head *region_list);