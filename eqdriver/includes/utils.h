#pragma once

#define INFO(args...)                                                          \
	do                                                                         \
	{                                                                          \
		pr_err("[INFO] " args);                                             \
	} while (0)

#define WARNING(args...)                                                       \
	do                                                                         \
	{                                                                          \
		pr_err("[WARNING] " args);                                           \
	} while (0)

#define ERROR(args...)                                                         \
	do                                                                         \
	{                                                                          \
		pr_err("[ERROR] " args);                                             \
	} while (0)
