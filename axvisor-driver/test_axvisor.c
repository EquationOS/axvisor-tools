#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main()
{
	const char *device_path = "/dev/axvisor_vdev";
	char buffer[128];
	ssize_t bytes_read;

	int fd = open(device_path, O_RDONLY);
	if (fd < 0)
	{
		perror("Failed to open device");
		return 1;
	}

	bytes_read = read(fd, buffer, sizeof(buffer) - 1);
	if (bytes_read < 0)
	{
		perror("Failed to read from device");
		close(fd);
		return 1;
	}

	buffer[bytes_read] = '\0';

	printf("Read from device: %s", buffer);

	close(fd);
	return 0;
}