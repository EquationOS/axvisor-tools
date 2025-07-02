#include "includes/shm.h"

/**
 * Find the first free bit in the bitmap and return its position.
 * Returns -1 if no free bit is found.
 */
static int find_free_bit(uint64_t *bitmaps, int size)
{
	if (!bitmaps)
	{
		return -1; // Invalid bitmaps pointer
	}
	if (size <= 0 || size > BITMAP_SIZE)
	{
		return -1; // Size exceeds the maximum supported size
	}
	for (int i = 0; i < size; i++)
	{
		if (bitmaps[i] != UINT64_MAX) // Check if there is any free bit
		{
			for (int j = 0; j < 64; j++)
			{
				if (!(bitmaps[i] & (1ULL << j))) // Check if the bit is free
				{
					return i * 64 + j;
				}
			}
		}
	}
	return -1; // No free bit found
}

/**
 * Allocate a 4KB page from the eqshm region.
 * Returns the base address of the allocated page or NULL if allocation fails.
 */
void *allocate_shm_page(eqshm_t *region)
{
	int pos = find_free_bit(region->bitmaps, BITMAP_SIZE);
	if (pos == -1)
	{
		return NULL; // No free page available
	}

	region->bitmaps[pos / 64] |=
		(1ULL << (pos % 64)); // Mark the bit as allocated
	return region->base +
		   pos * PAGE_SIZE; // Calculate the base address of the allocated page
}

/**
 * Free a 4KB page in the eqshm region.
 */
void free_shm_page(eqshm_t *region, void *page)
{
	uint64_t offset = (page - region->base) / PAGE_SIZE;
	region->bitmaps[offset / 64] &=
		~(1ULL << (offset % 64)); // Mark the bit as free
}

/**
 * Allocate multiple contiguous 4KB pages from the eqshm region.
 * Returns the base address of the allocated pages or NULL if allocation fails.
 */
void *allocate_contiguous_shm_pages(eqshm_t *region, int page_num)
{
	if (page_num <= 0)
	{
		return NULL; // Invalid number of pages
	}

	int start_pos = -1;
	int count = 0;

	// Search for contiguous free pages
	for (int i = 0; i < 8 * 64; i++)
	{
		int bitmap_idx = i / 64;
		int bit_idx = i % 64;

		if (!(region->bitmaps[bitmap_idx] & (1ULL << bit_idx)))
		{
			if (count == 0)
			{
				start_pos = i; // Start of the contiguous block
			}
			count++;
			if (count == page_num)
			{
				break; // Found enough contiguous pages
			}
		}
		else
		{
			count = 0; // Reset count if a used page is encountered
		}
	}

	if (count < page_num)
	{
		return NULL; // Not enough contiguous pages available
	}

	// Mark the pages as allocated
	for (int i = 0; i < page_num; i++)
	{
		int pos = start_pos + i;
		region->bitmaps[pos / 64] |= (1ULL << (pos % 64));
	}

	return region->base +
		   start_pos *
			   PAGE_SIZE; // Calculate the base address of the allocated pages
}

/**
 * Free multiple contiguous 4KB pages in the eqshm region.
 */
void free_contiguous_shm_pages(eqshm_t *region, void *page, int page_num)
{
	if (page_num <= 0)
	{
		return; // Invalid number of pages
	}

	uint64_t start_offset = (page - region->base) / PAGE_SIZE;

	// Mark the pages as free
	for (int i = 0; i < page_num; i++)
	{
		uint64_t offset = start_offset + i;
		region->bitmaps[offset / 64] &= ~(1ULL << (offset % 64));
	}
}

/**
 * Check if all pages in the eqshm region are free.
 */
int is_shm_region_empty(eqshm_t *region)
{
	for (int i = 0; i < 8; i++)
	{
		if (region->bitmaps[i] != 0)
		{
			return 0; // Not empty
		}
	}
	return 1; // Empty
}

/**
 * Check if all pages in the eqshm region are allocated.
 */
int is_shm_region_full(eqshm_t *region)
{
	for (int i = 0; i < 8; i++)
	{
		if (region->bitmaps[i] != UINT64_MAX)
		{
			return 0; // Not full
		}
	}
	return 1; // Full
}

/**
 * Allocate a new eqshm region if the current region is full.
 */
eqshm_t *allocate_new_shm_region(struct list_head *region_list, void *base)
{
	eqshm_t *new_region = kzalloc(sizeof(eqshm_t), GFP_KERNEL);
	if (!new_region)
	{
		return NULL; // Allocation failed
	}

	new_region->base = base;
	memset(new_region->bitmaps, 0, sizeof(new_region->bitmaps));
	INIT_LIST_HEAD(&new_region->list);
	list_add_tail(&new_region->list, region_list);

	return new_region;
}

/**
 * Release the eqshm region if all pages are free.
 */
void release_shm_region(eqshm_t *region, struct list_head *region_list)
{
	if (is_shm_region_empty(region))
	{
		list_del(&region->list);
		kfree(region);
	}
}

/**
 * Clearup the eqshm region even if it is not empty.
 */
void cleanup_shm_region(eqshm_t *region, struct list_head *region_list)
{
	list_del(&region->list);
	kfree(region);
}

eqscf_queue_region_t *get_scf_queue_region_by_host_pid(
	struct list_head *scf_region_list, uint64_t host_pid)
{
	for (eqscf_queue_region_t *region = list_first_entry_or_null(
			 scf_region_list, eqscf_queue_region_t, list);
		 region != NULL; region = list_next_entry(region, list))
	{
		if (region->host_pid == host_pid)
		{
			return region; // Found the SCF queue region for the given PID
		}
	}
	return NULL; // Not found
}