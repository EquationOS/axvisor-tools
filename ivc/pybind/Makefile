# Basic information
LIB_NAME 		:= ivc

# Compilation settings
## Cross-compilation is not supported yet here
CXXCOMP 		?= g++

# Input and output directories
MAKEFILE_DIR 	:= $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
PARENT_DIR 		:= $(abspath $(MAKEFILE_DIR)/..)/

BUILD_DIR 		?= $(PARENT_DIR)build
VENV_DIR 		:= $(BUILD_DIR)/venv

# Python commands
PYTHON_GLOBAL	?= python3
VENV			?= $(PYTHON_GLOBAL) -m venv
PYTHON		 	?= $(VENV_DIR)/bin/python
PIP				?= $(VENV_DIR)/bin/pip

# Python bindings
PYBIND_LIB_NAME := $(LIB_NAME)py
PYBIND_SRC		:= $(MAKEFILE_DIR)ivc_pybind.cpp
PYBIND_LIB		:= $(BUILD_DIR)/$(PYBIND_LIB_NAME).so

.PHONY: pybind venv test
pybind: venv $(PYBIND_LIB)

$(PYBIND_LIB): $(PYBIND_SRC)
	@echo "Building Python bindings for $(LIB_NAME)..."
	$(CXXCOMP) -std=c++11 -shared -fPIC -o $@ $< $(BUILD_DIR)/lib$(LIB_NAME).a -I$(PARENT_DIR)include $(shell $(PYTHON) -m pybind11 --includes) 
	@echo "Python bindings built successfully."

venv: $(PYTHON)
$(PYTHON):
	@echo "Creating virtual environment in $(VENV_DIR)..."
	@$(VENV) $(VENV_DIR)
	@echo "Virtual environment created. Installing dependencies..."
	@$(PIP) install -r $(MAKEFILE_DIR)requirements.txt
	@echo "Dependencies installed."

test: $(PYBIND_LIB)
	@echo "Launching Python..."
	cd $(BUILD_DIR) && $(PYTHON) -i -c "import $(PYBIND_LIB_NAME); print('Python bindings for $(LIB_NAME) loaded successfully!')"
	@echo "Test completed successfully."
